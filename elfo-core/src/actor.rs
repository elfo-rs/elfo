use parking_lot::RwLock;

use crate::{
    addr::Addr,
    context::Context,
    envelope::Envelope,
    errors::{SendError, TrySendError},
    mailbox::Mailbox,
    request_table::RequestTable,
    supervisor::RouteReport,
};

pub(crate) struct Actor {
    mailbox: Mailbox,
    request_table: RequestTable,
    control: RwLock<ControlBlock>,
}

struct ControlBlock {
    status: ActorStatus,
    details: Option<String>,
}

#[derive(Clone)]
pub enum ActorStatus {
    Initializing,
    Alarming,
    Normal,
    Restarting,
}

impl Actor {
    pub(crate) fn new(addr: Addr) -> Self {
        Actor {
            mailbox: Mailbox::new(),
            request_table: RequestTable::new(addr),
            control: RwLock::new(ControlBlock {
                status: ActorStatus::Initializing,
                details: None,
            }),
        }
    }

    pub(crate) fn try_send(&self, envelope: Envelope) -> Result<(), TrySendError<Envelope>> {
        if self.is_restarting() {
            return Err(TrySendError::Closed(envelope));
        }
        self.mailbox().try_send(envelope)
    }

    pub(crate) fn mailbox(&self) -> &Mailbox {
        &self.mailbox
    }

    pub(crate) fn request_table(&self) -> &RequestTable {
        &self.request_table
    }

    pub(crate) fn set_status(&self, status: ActorStatus, details: Option<String>) {
        let mut control = self.control.write();
        control.status = status;
        control.details = details;
    }

    pub(crate) fn is_restarting(&self) -> bool {
        matches!(self.control.read().status, ActorStatus::Restarting)
    }
}
