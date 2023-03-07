use std::{fmt, mem, sync::Arc};

use futures_intrusive::sync::ManualResetEvent;
use metrics::{decrement_gauge, increment_counter, increment_gauge};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use tracing::{error, info, warn};

use crate::{
    addr::Addr,
    envelope::Envelope,
    errors::{SendError, TrySendError},
    group::TerminationPolicy,
    mailbox::{Mailbox, RecvResult},
    messages::{ActorStatusReport, Terminate},
    request_table::RequestTable,
    scope,
    subscription::SubscriptionManager,
};

use crate::{self as elfo};
use elfo_macros::msg_raw as msg;

// === ActorMeta ===

/// Represents meta information about actor: his group and key.
#[derive(Debug, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActorMeta {
    pub group: String,
    pub key: String,
}

// === ActorStatus ===

/// Represents the current status of an actor.
/// See [The Actoromicon](https://actoromicon.rs/ch03-01-actor-lifecycle.html) for details.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActorStatus {
    kind: ActorStatusKind,
    details: Option<String>,
}

impl ActorStatus {
    pub const ALARMING: ActorStatus = ActorStatus::new(ActorStatusKind::Alarming);
    pub(crate) const FAILED: ActorStatus = ActorStatus::new(ActorStatusKind::Failed);
    pub const INITIALIZING: ActorStatus = ActorStatus::new(ActorStatusKind::Initializing);
    pub const NORMAL: ActorStatus = ActorStatus::new(ActorStatusKind::Normal);
    pub(crate) const TERMINATED: ActorStatus = ActorStatus::new(ActorStatusKind::Terminated);
    pub const TERMINATING: ActorStatus = ActorStatus::new(ActorStatusKind::Terminating);

    const fn new(kind: ActorStatusKind) -> Self {
        Self {
            kind,
            details: None,
        }
    }

    /// Creates a new status with the same kind and provided details.
    pub fn with_details(&self, details: impl fmt::Display) -> Self {
        ActorStatus {
            kind: self.kind,
            details: Some(details.to_string()),
        }
    }

    /// Returns the corresponding [`ActorStatusKind`] for this status.
    pub fn kind(&self) -> ActorStatusKind {
        self.kind
    }

    /// Returns details for this status, if provided.
    pub fn details(&self) -> Option<&str> {
        self.details.as_deref()
    }

    pub(crate) fn is_failed(&self) -> bool {
        self.kind == ActorStatusKind::Failed
    }

    fn is_finished(&self) -> bool {
        use ActorStatusKind::*;
        matches!(self.kind, Failed | Terminated)
    }
}

impl fmt::Display for ActorStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.details {
            Some(details) => write!(f, "{:?}: {}", self.kind, details),
            None => write!(f, "{:?}", self.kind),
        }
    }
}

// === ActorStatusKind ===

/// A list specifying statuses of actors. It's used with the [`ActorStatus`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum ActorStatusKind {
    Normal,
    Initializing,
    Terminating,
    Terminated,
    Alarming,
    Failed,
}

impl ActorStatusKind {
    fn as_str(&self) -> &'static str {
        match self {
            ActorStatusKind::Normal => "Normal",
            ActorStatusKind::Initializing => "Initializing",
            ActorStatusKind::Terminating => "Terminating",
            ActorStatusKind::Terminated => "Terminated",
            ActorStatusKind::Alarming => "Alarming",
            ActorStatusKind::Failed => "Failed",
        }
    }
}

// === Actor ===

pub(crate) struct Actor {
    meta: Arc<ActorMeta>,
    termination_policy: TerminationPolicy,
    mailbox: Mailbox,
    request_table: RequestTable,
    control: RwLock<ControlBlock>,
    finished: ManualResetEvent, // TODO: remove in favor of `status_subscription`?
    status_subscription: Arc<SubscriptionManager>,
}

struct ControlBlock {
    status: ActorStatus,
}

impl Actor {
    pub(crate) fn new(
        meta: Arc<ActorMeta>,
        addr: Addr,
        termination_policy: TerminationPolicy,
        status_subscription: Arc<SubscriptionManager>,
    ) -> Self {
        Actor {
            meta,
            termination_policy,
            mailbox: Mailbox::new(),
            request_table: RequestTable::new(addr),
            control: RwLock::new(ControlBlock {
                status: ActorStatus::INITIALIZING,
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

    pub(crate) fn try_send(&self, envelope: Envelope) -> Result<(), TrySendError<Envelope>> {
        msg!(match &envelope {
            Terminate { closing } => {
                if *closing || self.termination_policy.close_mailbox {
                    if self.close() {
                        return Ok(());
                    } else {
                        return Err(TrySendError::Closed(envelope));
                    }
                }
            }
        });

        self.mailbox.try_send(envelope)
    }

    pub(crate) async fn send(&self, envelope: Envelope) -> Result<(), SendError<Envelope>> {
        msg!(match &envelope {
            Terminate { closing } => {
                if *closing || self.termination_policy.close_mailbox {
                    if self.close() {
                        return Ok(());
                    } else {
                        return Err(SendError(envelope));
                    }
                }
            }
        });

        self.mailbox.send(envelope).await
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

    // Note that this method should be called inside a right scope.
    pub(crate) fn set_status(&self, status: ActorStatus) {
        let mut control = self.control.write();
        let prev_status = mem::replace(&mut control.status, status.clone());

        if status == prev_status {
            return;
        }

        self.send_status_to_subscribers(&control);
        drop(control);

        if status.is_finished() {
            self.close();
            // Drop all messages to release requests immediately.
            self.mailbox.drop_all();
            self.finished.set();
        }

        log_status(&status);

        if status.kind != prev_status.kind {
            if !prev_status.is_finished() {
                decrement_gauge!("elfo_active_actors", 1., "status" => prev_status.kind.as_str());
            }
            if !status.is_finished() {
                increment_gauge!("elfo_active_actors", 1., "status" => status.kind.as_str());
            }

            increment_counter!("elfo_actor_status_changes_total", "status" => status.kind.as_str());
        }

        // TODO: use `sdnotify` to provide a detailed status to systemd.
        //       or use another actor to listen all statuses for this.
    }

    pub(crate) fn close(&self) -> bool {
        self.mailbox.close(scope::trace_id())
    }

    pub(crate) fn is_initializing(&self) -> bool {
        matches!(
            self.control.read().status.kind,
            ActorStatusKind::Initializing
        )
    }

    pub(crate) fn is_terminating(&self) -> bool {
        matches!(
            self.control.read().status.kind,
            ActorStatusKind::Terminating
        )
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

    fn send_status_to_subscribers(&self, control: &ControlBlock) {
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
        assert!(actor.control.read().status.is_finished());
        actor.finished().await;
        assert!(actor.control.read().status.is_finished());
    }
}
