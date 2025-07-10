use std::{
    fmt, mem,
    sync::{atomic, Arc},
};

use futures_intrusive::sync::ManualResetEvent;
use metrics::{decrement_gauge, increment_counter, increment_gauge};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use tracing::{error, info, warn};

use crate::{
    actor_status::{ActorStatus, ActorStatusKind, AtomicActorStatusKind},
    envelope::Envelope,
    errors::{SendError, TrySendError},
    group::TerminationPolicy,
    mailbox::{config::MailboxConfig, Mailbox, RecvResult},
    messages::{ActorStatusReport, Terminate},
    msg,
    request_table::RequestTable,
    restarting::RestartPolicy,
    scope,
    subscription::SubscriptionManager,
    Addr,
};

// === ActorMeta ===

/// Represents meta information about actor: his group and key.
#[derive(Debug, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActorMeta {
    /// Actor's group name set in the topology.
    pub group: String,
    /// Actor's key set by the router.
    pub key: String,
}

impl fmt::Display for ActorMeta {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.group)?;

        if !self.key.is_empty() {
            f.write_str("/")?;
            f.write_str(&self.key)?;
        }

        Ok(())
    }
}

// === ActorStartInfo ===

/// A struct holding information related to an actor start.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ActorStartInfo {
    /// The cause for the actor start, indicating why the actor is being
    /// initialized.
    pub cause: ActorStartCause,
}

/// An enum representing various causes for an actor to start.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum ActorStartCause {
    /// The actor started because its group was mounted.
    GroupMounted,
    /// The actor started in response to a message.
    OnMessage,
    /// The actor started due to the restart policy.
    Restarted,
}

impl ActorStartInfo {
    pub(crate) fn on_group_mounted() -> Self {
        Self {
            cause: ActorStartCause::GroupMounted,
        }
    }

    pub(crate) fn on_message() -> Self {
        Self {
            cause: ActorStartCause::OnMessage,
        }
    }

    pub(crate) fn on_restart() -> Self {
        Self {
            cause: ActorStartCause::Restarted,
        }
    }
}

impl ActorStartCause {
    pub fn is_group_mounted(&self) -> bool {
        matches!(self, ActorStartCause::GroupMounted)
    }

    pub fn is_restarted(&self) -> bool {
        matches!(self, ActorStartCause::Restarted)
    }

    pub fn is_on_message(&self) -> bool {
        matches!(self, ActorStartCause::OnMessage)
    }
}

// === Actor ===

pub(crate) struct Actor {
    meta: Arc<ActorMeta>,
    termination_policy: TerminationPolicy,
    mailbox: Mailbox,
    request_table: RequestTable,
    status_kind: AtomicActorStatusKind,
    control: RwLock<Control>,
    finished: ManualResetEvent, // TODO: remove in favor of `status_subscription`?
    status_subscription: Arc<SubscriptionManager>,
}

struct Control {
    status: ActorStatus,
    /// If `None`, a group's policy will be used.
    restart_policy: Option<RestartPolicy>,
    /// A mailbox capacity set in the config.
    mailbox_capacity_config: usize,
    /// Explicitly set mailbox capacity via `Context::set_mailbox_capacity()`.
    mailbox_capacity_override: Option<usize>,
}

impl Actor {
    pub(crate) fn new(
        meta: Arc<ActorMeta>,
        addr: Addr,
        mailbox_config: &MailboxConfig,
        termination_policy: TerminationPolicy,
        status_subscription: Arc<SubscriptionManager>,
    ) -> Self {
        Actor {
            status_kind: AtomicActorStatusKind::from(ActorStatusKind::Initializing),
            meta,
            termination_policy,
            mailbox: Mailbox::new(mailbox_config),
            request_table: RequestTable::new(addr),
            control: RwLock::new(Control {
                status: ActorStatus::INITIALIZING,
                restart_policy: None,
                mailbox_capacity_config: mailbox_config.capacity,
                mailbox_capacity_override: None,
            }),
            finished: ManualResetEvent::new(false),
            status_subscription,
        }
    }

    pub(crate) fn on_start(&self) {
        increment_gauge!("elfo_active_actors", 1.,
            "status" => ActorStatusKind::Initializing.as_str());
        increment_counter!("elfo_actor_status_changes_total",
            "status" => ActorStatusKind::Initializing.as_str());

        self.send_status_to_subscribers(&self.control.read());
    }

    pub(crate) async fn send(&self, envelope: Envelope) -> Result<(), SendError<Envelope>> {
        match self.handle_system(envelope) {
            Some(envelope) => self.mailbox.send(envelope).await,
            None => Ok(()),
        }
    }

    pub(crate) fn try_send(&self, envelope: Envelope) -> Result<(), TrySendError<Envelope>> {
        match self.handle_system(envelope) {
            Some(envelope) => self.mailbox.try_send(envelope),
            None => Ok(()),
        }
    }

    pub(crate) fn unbounded_send(&self, envelope: Envelope) -> Result<(), SendError<Envelope>> {
        match self.handle_system(envelope) {
            Some(envelope) => self.mailbox.unbounded_send(envelope),
            None => Ok(()),
        }
    }

    #[inline(always)]
    fn handle_system(&self, envelope: Envelope) -> Option<Envelope> {
        msg!(match &envelope {
            Terminate { closing, .. } => {
                if (*closing || self.termination_policy.close_mailbox) && self.close() {
                    // First closing `Terminate` is considered successful.
                    return None;
                }
            }
        });

        // If the mailbox is closed, all following `*_send()` returns an error.
        Some(envelope)
    }

    pub(crate) async fn recv(&self) -> RecvResult {
        self.mailbox.recv().await
    }

    pub(crate) fn try_recv(&self) -> Option<RecvResult> {
        self.mailbox.try_recv()
    }

    pub(crate) fn request_table(&self) -> &RequestTable {
        &self.request_table
    }

    pub(crate) fn set_mailbox_capacity_config(&self, capacity: usize) {
        self.control.write().mailbox_capacity_config = capacity;
        self.update_mailbox_capacity();
    }

    pub(crate) fn set_mailbox_capacity_override(&self, capacity: Option<usize>) {
        self.control.write().mailbox_capacity_override = capacity;
        self.update_mailbox_capacity();
    }

    fn update_mailbox_capacity(&self) {
        let control = self.control.read();

        let capacity = control
            .mailbox_capacity_override
            .unwrap_or(control.mailbox_capacity_config);

        self.mailbox.set_capacity(capacity);
    }

    pub(crate) fn restart_policy(&self) -> Option<RestartPolicy> {
        self.control.read().restart_policy.clone()
    }

    pub(crate) fn set_restart_policy(&self, policy: Option<RestartPolicy>) {
        self.control.write().restart_policy = policy;
    }

    pub(crate) fn status_kind(&self) -> ActorStatusKind {
        self.status_kind.load(atomic::Ordering::Acquire)
    }

    // Note that this method should be called inside a right scope.
    pub(crate) fn set_status(&self, status: ActorStatus) {
        self.status_kind
            .store(status.kind(), atomic::Ordering::Release);

        let mut control = self.control.write();
        let prev_status = mem::replace(&mut control.status, status.clone());

        if status == prev_status {
            return;
        }

        self.send_status_to_subscribers(&control);
        drop(control);

        if status.kind().is_finished() {
            self.close();
            // Drop all messages to release requests immediately.
            self.mailbox.drop_all();
            self.finished.set();
        }

        log_status(&status);

        if status.kind != prev_status.kind {
            if !prev_status.kind().is_finished() {
                decrement_gauge!("elfo_active_actors", 1., "status" => prev_status.kind.as_str());
            }
            if !status.kind().is_finished() {
                increment_gauge!("elfo_active_actors", 1., "status" => status.kind.as_str());
            }

            increment_counter!("elfo_actor_status_changes_total", "status" => status.kind.as_str());
        }

        // TODO: use `sdnotify` to provide a detailed status to systemd.
        //       or use another actor to listen all statuses for this.
    }

    #[cold]
    #[inline(never)]
    pub(crate) fn close(&self) -> bool {
        self.mailbox.close(scope::trace_id())
    }

    pub(crate) async fn finished(&self) {
        self.finished.wait().await
    }

    /// Accesses the actor's status under lock to avoid race conditions.
    pub(crate) fn with_status<R>(&self, f: impl FnOnce(ActorStatusReport) -> R) -> R {
        let control = self.control.read();
        f(ActorStatusReport {
            meta: self.meta.clone(),
            status: control.status.clone(),
        })
    }

    fn send_status_to_subscribers(&self, control: &Control) {
        self.status_subscription.send(ActorStatusReport {
            meta: self.meta.clone(),
            status: control.status.clone(),
        });
    }
}

fn log_status(status: &ActorStatus) {
    if let Some(details) = status.details.as_deref() {
        match status.kind {
            ActorStatusKind::Failed => error!(status = ?status.kind, %details, "status changed"),
            ActorStatusKind::Alarming => warn!(status = ?status.kind, %details, "status changed"),
            _ => info!(status = ?status.kind, %details, "status changed"),
        }
    } else {
        match status.kind {
            ActorStatusKind::Failed => error!(status = ?status.kind, "status changed"),
            ActorStatusKind::Alarming => warn!(status = ?status.kind, "status changed"),
            _ => info!(status = ?status.kind, "status changed"),
        }
    }
}

#[cfg(test)]
#[cfg(feature = "FIXME")]
mod tests {
    use super::*;

    #[tokio::test]
    async fn finished() {
        let meta = Arc::new(ActorMeta {
            group: "foo".into(),
            key: "bar".into(),
        });

        let actor = Actor::new(meta, Addr::NULL, TerminationPolicy::default());
        let fut = actor.finished();
        actor.set_status(ActorStatus::TERMINATED);
        fut.await;
        assert!(actor.status_kind().is_finished());
        actor.finished().await;
        assert!(actor.status_kind().is_finished());
    }
}
