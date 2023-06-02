use derive_more::From;
use futures::future::{join_all, BoxFuture};

use crate::{
    actor::Actor,
    context::Context,
    envelope::Envelope,
    errors::{SendError, TrySendError},
    supervisor::RouteReport,
    Addr,
};

pub(crate) struct Object {
    addr: Addr,
    kind: ObjectKind,
}

assert_impl_all!(Object: Sync);
// TODO: actually, `Slab::Entry<Object>` should be aligned.
// assert_eq_size!(Object, [u8; 256]);

pub(crate) type ObjectRef<'a> = sharded_slab::Entry<'a, Object>;
pub(crate) type ObjectArc = sharded_slab::OwnedEntry<Object>;

#[derive(From)]
pub(crate) enum ObjectKind {
    Actor(Actor),
    Group(Group),
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

    pub(crate) async fn send<C, K>(
        &self,
        ctx: &Context<C, K>,
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
            ObjectKind::Actor(actor) => Some(actor),
            ObjectKind::Group(_) => None,
        }
    }

    pub(crate) async fn finished(&self) {
        match &self.kind {
            ObjectKind::Actor(actor) => actor.finished().await,
            ObjectKind::Group(group) => (group.finished)().await,
        }
    }
}

pub(crate) struct Group {
    router: GroupRouter,
    finished: GroupFinished,
}

impl Group {
    pub(crate) fn new(router: GroupRouter, finished: GroupFinished) -> Self {
        Group { router, finished }
    }
}

pub(crate) type GroupRouter = Box<dyn Fn(Envelope) -> RouteReport + Send + Sync>;
type GroupFinished = Box<dyn Fn() -> BoxFuture<'static, ()> + Send + Sync>;
