use std::{fmt, mem};

use futures_intrusive::sync::ManualResetEvent;
use metrics::{decrement_gauge, increment_counter, increment_gauge};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use tracing::{error, info};

use crate::{
    addr::Addr,
    envelope::Envelope,
    errors::{SendError, TryRecvError, TrySendError},
    group::TerminationPolicy,
    mailbox::Mailbox,
    messages::Terminate,
    request_table::RequestTable,
};

use crate::{self as elfo};
use elfo_macros::msg_raw as msg;

pub(crate) struct Actor {
    termination_policy: TerminationPolicy,
    mailbox: Mailbox,
    request_table: RequestTable,
    control: RwLock<ControlBlock>,
    finished: ManualResetEvent,
}

struct ControlBlock {
    status: ActorStatus,
}

#[derive(Debug, Hash, PartialEq, Serialize, Deserialize)]
pub struct ActorMeta {
    pub group: String,
    pub key: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActorStatus {
    kind: ActorStatusKind,
    details: Option<String>,
}

/// A list specifying statuses of actors. It's used with the [`ActorStatus`].
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
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

impl fmt::Display for ActorStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.details {
            Some(details) => write!(f, "{:?}: {}", self.kind, details),
            None => write!(f, "{:?}", self.kind),
        }
    }
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
            details: Some(format!("{}", details)),
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

impl Actor {
    pub(crate) fn new(addr: Addr, termination_policy: TerminationPolicy) -> Self {
        Actor {
            termination_policy,
            mailbox: Mailbox::new(),
            request_table: RequestTable::new(addr),
            control: RwLock::new(ControlBlock {
                status: ActorStatus::INITIALIZING,
            }),
            finished: ManualResetEvent::new(false),
        }
    }

    pub(crate) fn on_start(&self) {
        increment_gauge!("elfo_active_actors", 1.,
            "status" => ActorStatusKind::Initializing.as_str());
        increment_counter!("elfo_actor_status_changes_total",
            "status" => ActorStatusKind::Initializing.as_str());
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

    pub(crate) async fn recv(&self) -> Option<Envelope> {
        self.mailbox.recv().await
    }

    pub(crate) fn try_recv(&self) -> Result<Envelope, TryRecvError> {
        self.mailbox.try_recv()
    }

    pub(crate) fn request_table(&self) -> &RequestTable {
        &self.request_table
    }

    // Note that this method should be called inside a right scope.
    pub(crate) fn set_status(&self, status: ActorStatus) {
        let mut control = self.control.write();
        let prev_status = mem::replace(&mut control.status, status.clone());
        drop(control);

        if status == prev_status {
            return;
        }

        if status.is_finished() {
            self.mailbox.close();
            self.finished.set();
        }

        let is_bad_kind = matches!(
            status.kind,
            ActorStatusKind::Alarming | ActorStatusKind::Failed
        );

        if let Some(details) = status.details.as_deref() {
            if is_bad_kind {
                error!(status = ?status.kind, %details, "status changed");
            } else {
                info!(status = ?status.kind, %details, "status changed");
            }
        } else if is_bad_kind {
            error!(status = ?status.kind, "status changed");
        } else {
            info!(status = ?status.kind, "status changed");
        };

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
    }

    pub(crate) fn close(&self) -> bool {
        self.mailbox.close()
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn finished() {
        let actor = Actor::new(Addr::NULL, TerminationPolicy::default());
        let fut = actor.finished();
        actor.set_status(ActorStatus::TERMINATED);
        fut.await;
        assert!(actor.control.read().status.is_finished());
        actor.finished().await;
        assert!(actor.control.read().status.is_finished());
    }
}
