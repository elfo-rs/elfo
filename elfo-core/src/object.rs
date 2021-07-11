use derive_more::From;
use futures::future::join_all;
use smallbox::SmallBox;

use crate::{
    actor::Actor,
    addr::Addr,
    context::Context,
    envelope::Envelope,
    errors::{SendError, TrySendError},
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

#[derive(From)]
pub(crate) enum ObjectKind {
    Actor(Actor),
    Group(Group),
}

#[derive(Debug, Hash, PartialEq)]
pub struct ObjectMeta {
    pub group: String,
    pub key: Option<String>,
}

impl Object {
    pub(crate) fn new(addr: Addr, kind: impl Into<ObjectKind>) -> Self {
        Self {
            addr,
            kind: kind.into(),
        }
    }

    pub(crate) fn addr(&self) -> Addr {
        self.addr
    }

    pub(crate) async fn send<C, K, S>(
        &self,
        ctx: &Context<C, K, S>,
        envelope: Envelope,
    ) -> Result<(), SendError<Envelope>> {
        match &self.kind {
            ObjectKind::Actor(handle) => handle.send(envelope).await,
            ObjectKind::Group(handle) => match (handle.router)(envelope) {
                RouteReport::Done => Ok(()),
                RouteReport::Wait(addr, envelope) => match ctx.book().get_owned(addr) {
                    Some(object) => {
                        object
                            .as_actor()
                            .expect("supervisor stores only actors")
                            .send(envelope)
                            .await
                    }
                    None => Err(SendError(envelope)),
                },
                RouteReport::WaitAll(someone, pairs) => {
                    debug_assert_ne!(pairs.len(), 0);

                    let mut futures = Vec::new();

                    for (addr, envelope) in pairs {
                        let object = ctx.book().get_owned(addr);
                        futures.push(async move {
                            match object {
                                Some(object) => {
                                    object
                                        .as_actor()
                                        .expect("supervisor stores only actors")
                                        .send(envelope)
                                        .await
                                }
                                None => Err(SendError(envelope)),
                            }
                        });
                    }

                    let mut results = join_all(futures).await;

                    if someone || results.iter().any(Result::is_ok) {
                        Ok(())
                    } else {
                        results.pop().expect("empty pairs")
                    }
                }
                RouteReport::Closed(envelope) => Err(SendError(envelope)),
            },
        }
    }

    pub(crate) fn try_send(&self, envelope: Envelope) -> Result<(), TrySendError<Envelope>> {
        match &self.kind {
            ObjectKind::Actor(handle) => handle.try_send(envelope),
            ObjectKind::Group(handle) => match (handle.router)(envelope) {
                RouteReport::Done => Ok(()),
                RouteReport::Wait(_, envelope) => Err(TrySendError::Full(envelope)),
                RouteReport::WaitAll(someone, _) if someone => Ok(()),
                RouteReport::WaitAll(_, mut pairs) => {
                    Err(TrySendError::Full(pairs.pop().expect("empty pairs").1))
                }
                RouteReport::Closed(envelope) => Err(TrySendError::Closed(envelope)),
            },
        }
    }

    pub(crate) fn as_actor(&self) -> Option<&Actor> {
        match &self.kind {
            ObjectKind::Actor(actor) => Some(&actor),
            ObjectKind::Group(_) => None,
        }
    }
}

pub(crate) struct Group {
    router: GroupRouter,
}

impl Group {
    pub(crate) fn new(router: GroupRouter) -> Self {
        Group { router }
    }
}

pub(crate) type GroupRouter = SmallBox<dyn Fn(Envelope) -> RouteReport + Send + Sync, [u8; 220]>;
