use tokio::{sync::Mutex, task::JoinHandle};

use crate::{
    envelope::{Envelope, Message},
    mailbox::{Mailbox, SendError},
};

pub(crate) struct Object {
    kind: ObjectKind,
}

enum ObjectKind {
    Actor {
        mailbox: Mailbox,
        handle: Mutex<JoinHandle<()>>,
    },
    Group {
        // TODO: supervisor
    },
}

impl Object {
    pub(crate) fn new_actor(handle: JoinHandle<()>) -> Self {
        Self {
            kind: ObjectKind::Actor {
                mailbox: Mailbox::new(),
                handle: Mutex::new(handle),
            },
        }
    }

    pub(crate) fn new_group(supervisor: ()) -> Self {
        Self {
            kind: ObjectKind::Group {},
        }
    }

    pub(crate) async fn send<M: Message>(
        &self,
        envelope: Envelope<M>,
    ) -> Result<(), SendError<Envelope<M>>> {
        match &self.kind {
            ObjectKind::Actor { mailbox, .. } => mailbox.send(envelope).await,
            ObjectKind::Group {} => todo!(),
        }
    }

    pub(crate) fn mailbox(&self) -> Option<&Mailbox> {
        match &self.kind {
            ObjectKind::Actor { mailbox, .. } => Some(mailbox),
            ObjectKind::Group { .. } => todo!(),
        }
    }

    pub(crate) fn supervisor(&self) -> Option<&()> {
        match &self.kind {
            ObjectKind::Actor { .. } => None,
            ObjectKind::Group { .. } => todo!(),
        }
    }
}

#[test]
fn object_size() {
    // Force the size in order to avoid false sharing.
    assert_eq!(std::mem::size_of::<Object>(), 256);
}
