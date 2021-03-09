use futures::future::join_all;
use tokio::{sync::Mutex, task::JoinHandle};

use crate::{
    addr::Addr,
    context::Context,
    envelope::Envelope,
    group::GroupRouter,
    mailbox::{Mailbox, SendError},
    supervisor::RouteReport,
};

pub(crate) struct Object {
    addr: Addr,
    kind: ObjectKind,
}

assert_impl_all!(Object: Sync);
assert_eq_size!(Object, [u8; 256]);

pub(crate) type ObjectRef<'a> = sharded_slab::Entry<'a, Object>;
pub(crate) type ObjectArc = sharded_slab::OwnedEntry<Object>;

enum ObjectKind {
    Actor(ActorHandle),
    Group(GroupHandle),
}

impl Object {
    pub(crate) fn new_actor(addr: Addr, task: JoinHandle<()>) -> Self {
        let handle = ActorHandle {
            mailbox: Mailbox::new(),
            task: Mutex::new(task),
        };

        Self {
            addr,
            kind: ObjectKind::Actor(handle),
        }
    }

    pub(crate) fn new_group(addr: Addr, router: GroupRouter) -> Self {
        let handle = GroupHandle { router };

        Self {
            addr,
            kind: ObjectKind::Group(handle),
        }
    }

    pub(crate) fn addr(&self) -> Addr {
        self.addr
    }

    pub(crate) async fn send<C, K>(
        &self,
        ctx: &Context<C, K>,
        envelope: Envelope,
    ) -> Result<(), SendError<Envelope>> {
        match &self.kind {
            ObjectKind::Actor(handle) => handle.mailbox.send(envelope).await,
            ObjectKind::Group(handle) => match (handle.router)(envelope) {
                RouteReport::Done => Ok(()),
                RouteReport::Wait(addr, envelope) => match ctx.book().get_owned(addr) {
                    Some(object) => {
                        object
                            .mailbox()
                            .expect("supervisor stores only actors")
                            .send(envelope)
                            .await
                    }
                    None => Err(SendError(envelope)),
                },
                RouteReport::WaitAll(pairs) => {
                    debug_assert_ne!(pairs.len(), 0);

                    let mut futures = Vec::new();

                    for (addr, envelope) in pairs {
                        let object = ctx.book().get_owned(addr);
                        futures.push(async move {
                            match object {
                                Some(object) => {
                                    object
                                        .mailbox()
                                        .expect("supervisor stores only actors")
                                        .send(envelope)
                                        .await
                                }
                                None => Err(SendError(envelope)),
                            }
                        });
                    }

                    join_all(futures)
                        .await
                        .into_iter()
                        .find(Result::is_err)
                        .unwrap_or(Ok(()))
                }
                RouteReport::Closed(_) => Ok(()),
            },
        }
    }

    pub(crate) fn mailbox(&self) -> Option<&Mailbox> {
        match &self.kind {
            ObjectKind::Actor(handle) => Some(&handle.mailbox),
            ObjectKind::Group(_) => None,
        }
    }
}

struct ActorHandle {
    mailbox: Mailbox,
    task: Mutex<JoinHandle<()>>,
}

struct GroupHandle {
    router: GroupRouter,
}
