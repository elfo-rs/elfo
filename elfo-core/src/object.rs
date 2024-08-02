use std::{
    future::Future,
    mem,
    pin::Pin,
    task::{self, Poll},
};

use derive_more::From;
use futures::future::{join_all, BoxFuture};
use idr_ebr::{BorrowedEntry, OwnedEntry};
use pin_project::pin_project;
use smallvec::SmallVec;

#[cfg(feature = "network")]
use crate::remote::{self, RemoteHandle};
use crate::{
    actor::Actor,
    addr::Addr,
    envelope::Envelope,
    errors::{RequestError, SendError, TrySendError},
    request_table::ResponseToken,
};

// Reexported in `_priv`.
pub struct Object {
    addr: Addr,
    kind: ObjectKind,
}

assert_impl_all!(Object: Sync);

pub(crate) type BorrowedObject<'g> = BorrowedEntry<'g, Object>;
// Reexported in `_priv`.
pub type OwnedObject = OwnedEntry<Object>;

#[derive(From)]
pub(crate) enum ObjectKind {
    Actor(Actor),
    Group(Box<dyn GroupHandle>),
    #[cfg(feature = "network")]
    Remote(Box<dyn RemoteHandle>),
}

impl Object {
    pub(crate) fn new(addr: Addr, kind: impl Into<ObjectKind>) -> Self {
        Self {
            addr,
            kind: kind.into(),
        }
    }

    #[stability::unstable]
    #[inline]
    pub fn addr(&self) -> Addr {
        self.addr
    }

    // Tries to send an envelope to the object synchronously.
    // Only if the object is full, it gets an owned link to the object
    // (because an EBR guard cannot be hold over the async boundary)
    // and sends the envelope asynchronously.
    #[stability::unstable]
    pub fn send(
        this: BorrowedObject<'_>,
        recipient: Addr,
        envelope: Envelope,
    ) -> impl Future<Output = SendResult> + 'static {
        match &this.kind {
            ObjectKind::Actor(handle) => match handle.try_send(envelope) {
                Ok(()) => SendFut::Ready(Ok(())),
                Err(TrySendError::Closed(envelope)) => SendFut::Ready(Err(SendError(envelope))),
                Err(TrySendError::Full(envelope)) => {
                    let Some(this) = this.to_owned() else {
                        return SendFut::Ready(Err(SendError(envelope)));
                    };

                    SendFut::WaitActor(async move {
                        let actor = this.as_actor().unwrap();
                        actor.send(envelope).await
                    })
                }
            },
            ObjectKind::Group(handle) => {
                let mut visitor = SendGroupVisitor::default();
                handle.handle(envelope, &mut visitor);
                SendFut::WaitGroup(visitor.finish())
            }
            #[cfg(feature = "network")]
            ObjectKind::Remote(handle) => match handle.try_send(recipient, envelope) {
                Ok(()) => SendFut::Ready(Ok(())),
                Err(TrySendError::Closed(envelope)) => SendFut::Ready(Err(SendError(envelope))),
                Err(TrySendError::Full(mut envelope)) => {
                    let Some(this) = this.to_owned() else {
                        return SendFut::Ready(Err(SendError(envelope)));
                    };

                    SendFut::WaitRemote(async move {
                        let handle = this.as_remote().unwrap();
                        loop {
                            match handle.send(recipient, envelope) {
                                remote::SendResult::Ok => break Ok(()),
                                remote::SendResult::Err(err) => break Err(err),
                                remote::SendResult::Wait(notified, e) => {
                                    envelope = e;
                                    notified.await;
                                }
                            }
                        }
                    })
                }
            },
        }
    }

    #[stability::unstable]
    pub fn try_send(
        &self,
        recipient: Addr,
        envelope: Envelope,
    ) -> Result<(), TrySendError<Envelope>> {
        match &self.kind {
            ObjectKind::Actor(handle) => handle.try_send(envelope),
            ObjectKind::Group(handle) => {
                let mut visitor = TrySendGroupVisitor::default();
                handle.handle(envelope, &mut visitor);
                visitor.finish()
            }
            #[cfg(feature = "network")]
            ObjectKind::Remote(handle) => handle.try_send(recipient, envelope),
        }
    }

    #[stability::unstable]
    pub fn unbounded_send(
        &self,
        recipient: Addr,
        envelope: Envelope,
    ) -> Result<(), SendError<Envelope>> {
        match &self.kind {
            ObjectKind::Actor(handle) => handle.unbounded_send(envelope),
            ObjectKind::Group(handle) => {
                let mut visitor = UnboundedSendGroupVisitor::default();
                handle.handle(envelope, &mut visitor);
                visitor.finish()
            }
            #[cfg(feature = "network")]
            ObjectKind::Remote(handle) => handle.unbounded_send(recipient, envelope),
        }
    }

    #[stability::unstable]
    pub fn respond(&self, token: ResponseToken, response: Result<Envelope, RequestError>) {
        match &self.kind {
            ObjectKind::Actor(handle) => handle.request_table().resolve(token, response),
            ObjectKind::Group(_handle) => unreachable!(),
            #[cfg(feature = "network")]
            ObjectKind::Remote(handle) => handle.respond(token, response),
        }
    }

    #[stability::unstable]
    pub fn visit_group(&self, envelope: Envelope, visitor: &mut dyn GroupVisitor) {
        let ObjectKind::Group(handle) = &self.kind else {
            panic!("route() called on a non-group object");
        };

        handle.handle(envelope, visitor);
    }

    pub(crate) fn as_actor(&self) -> Option<&Actor> {
        match &self.kind {
            ObjectKind::Actor(handle) => Some(handle),
            _ => None,
        }
    }

    #[cfg(feature = "network")]
    #[allow(clippy::borrowed_box)]
    fn as_remote(&self) -> Option<&Box<dyn RemoteHandle>> {
        match &self.kind {
            ObjectKind::Remote(handle) => Some(handle),
            _ => None,
        }
    }

    pub(crate) async fn finished(&self) {
        match &self.kind {
            ObjectKind::Actor(actor) => actor.finished().await,
            ObjectKind::Group(group) => group.finished().await,
            #[cfg(feature = "network")]
            ObjectKind::Remote(_) => todo!(),
        }
    }
}

// === SendFut ===

type SendResult = Result<(), SendError<Envelope>>;

#[cfg(not(feature = "network"))]
#[pin_project(project = SendFutProj)]
enum SendFut<A, G> {
    Ready(SendResult),
    WaitActor(#[pin] A),
    WaitGroup(#[pin] G),
}

#[cfg(not(feature = "network"))]
impl<A, G> Future for SendFut<A, G>
where
    A: Future<Output = SendResult>,
    G: Future<Output = SendResult>,
{
    type Output = SendResult;

    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<Self::Output> {
        match self.project() {
            SendFutProj::Ready(result) => Poll::Ready(mem::replace(result, Ok(()))),
            SendFutProj::WaitActor(fut) => fut.poll(cx),
            SendFutProj::WaitGroup(fut) => fut.poll(cx),
        }
    }
}

#[cfg(feature = "network")]
#[pin_project(project = SendFutProj)]
enum SendFut<A, G, R> {
    Ready(SendResult),
    WaitActor(#[pin] A),
    WaitGroup(#[pin] G),
    WaitRemote(#[pin] R),
}

#[cfg(feature = "network")]
impl<A, G, R> Future for SendFut<A, G, R>
where
    A: Future<Output = SendResult>,
    G: Future<Output = SendResult>,
    R: Future<Output = SendResult>,
{
    type Output = SendResult;

    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<Self::Output> {
        match self.project() {
            SendFutProj::Ready(result) => Poll::Ready(mem::replace(result, Ok(()))),
            SendFutProj::WaitActor(fut) => fut.poll(cx),
            SendFutProj::WaitGroup(fut) => fut.poll(cx),
            SendFutProj::WaitRemote(fut) => fut.poll(cx),
        }
    }
}

pub(crate) trait GroupHandle: Send + Sync + 'static {
    fn handle(&self, envelope: Envelope, visitor: &mut dyn GroupVisitor);
    fn finished(&self) -> BoxFuture<'static, ()>;
}

/// The visitor of actors inside a group.
/// Possible sequences of calls:
/// * `done()`, if handled by a supervisor
/// * `empty()`, if no relevant actors in a group
/// * `visit_last()`, if only one relevant actor in a group
/// * `visit()`, `visit()`, .., `visit_last()`
pub trait GroupVisitor {
    fn done(&mut self);
    fn empty(&mut self, envelope: Envelope);
    fn visit(&mut self, object: &OwnedObject, envelope: &Envelope);
    fn visit_last(&mut self, object: &OwnedObject, envelope: Envelope);
}

// === SendGroupVisitor ===

#[derive(Default)]
struct SendGroupVisitor {
    extra: Option<Envelope>,
    full: SmallVec<[(OwnedObject, Envelope); 1]>,
    has_ok: bool,
}

impl SendGroupVisitor {
    // We must send while visiting to ensure that a message starting a new actor
    // is actually the first message that the actor receives.
    fn try_send(&mut self, object: &OwnedObject, envelope: Envelope) {
        let actor = object.as_actor().expect("group stores only actors");
        match actor.try_send(envelope) {
            Ok(()) => self.has_ok = true,
            Err(TrySendError::Full(envelope)) => {
                self.full.push((object.clone(), envelope));
            }
            Err(TrySendError::Closed(envelope)) => {
                self.extra = Some(envelope);
            }
        }
    }

    #[inline]
    async fn finish(mut self) -> SendResult {
        // Wait until messages reach all full actors.
        #[allow(clippy::comparison_chain)]
        if self.full.len() == 1 {
            let (object, envelope) = self.full.pop().unwrap();

            let actor = object.as_actor().expect("group stores only actors");
            match actor.send(envelope).await {
                Ok(()) => self.has_ok = true,
                Err(SendError(envelope)) => {
                    if !self.has_ok {
                        self.extra = Some(envelope);
                    }
                }
            }
        } else if self.full.len() > 1 {
            let mut futures = Vec::new();

            for (object, envelope) in self.full.drain(..) {
                futures.push(async move {
                    object
                        .as_actor()
                        .expect("group stores only actors")
                        .send(envelope)
                        .await
                });
            }

            for result in join_all(futures).await {
                match result {
                    Ok(()) => self.has_ok = true,
                    Err(SendError(envelope)) => {
                        if !self.has_ok {
                            self.extra = Some(envelope);
                        }
                    }
                }
            }
        }

        debug_assert!(self.full.is_empty());

        if self.has_ok {
            Ok(())
        } else {
            Err(SendError(self.extra.take().expect("missing envelope")))
        }
    }
}

impl GroupVisitor for SendGroupVisitor {
    fn done(&mut self) {
        debug_assert!(self.full.is_empty());
        debug_assert!(self.extra.is_none());
        debug_assert!(!self.has_ok);
        self.has_ok = true;
    }

    fn empty(&mut self, envelope: Envelope) {
        debug_assert!(self.full.is_empty());
        debug_assert!(self.extra.is_none());
        debug_assert!(!self.has_ok);
        self.extra = Some(envelope);
    }

    fn visit(&mut self, object: &OwnedObject, envelope: &Envelope) {
        let envelope = self.extra.take().unwrap_or_else(|| envelope.duplicate());
        self.try_send(object, envelope);
    }

    fn visit_last(&mut self, object: &OwnedObject, envelope: Envelope) {
        self.try_send(object, envelope);
    }
}

// === TrySendGroupVisitor ===

#[derive(Default)]
struct TrySendGroupVisitor {
    extra: Option<Envelope>,
    has_ok: bool,
    has_full: bool,
}

impl TrySendGroupVisitor {
    // We must send while visiting to ensure that a message starting a new actor
    // is actually the first message that the actor receives.
    fn try_send(&mut self, object: &OwnedObject, envelope: Envelope) {
        let actor = object.as_actor().expect("group stores only actors");
        match actor.try_send(envelope) {
            Ok(()) => self.has_ok = true,
            Err(err) => {
                if err.is_full() {
                    self.has_full = true;
                }
                self.extra = Some(err.into_inner());
            }
        }
    }

    fn finish(mut self) -> Result<(), TrySendError<Envelope>> {
        if self.has_ok {
            Ok(())
        } else {
            let envelope = self.extra.take().expect("missing envelope");
            Err(if self.has_full {
                TrySendError::Full(envelope)
            } else {
                TrySendError::Closed(envelope)
            })
        }
    }
}

impl GroupVisitor for TrySendGroupVisitor {
    fn done(&mut self) {
        debug_assert!(self.extra.is_none());
        debug_assert!(!self.has_ok);
        self.has_ok = true;
    }

    fn empty(&mut self, envelope: Envelope) {
        debug_assert!(self.extra.is_none());
        debug_assert!(!self.has_ok);
        self.extra = Some(envelope);
    }

    fn visit(&mut self, object: &OwnedObject, envelope: &Envelope) {
        let envelope = self.extra.take().unwrap_or_else(|| envelope.duplicate());
        self.try_send(object, envelope);
    }

    fn visit_last(&mut self, object: &OwnedObject, envelope: Envelope) {
        self.try_send(object, envelope);
    }
}

// === UnboundedSendGroupVisitor ===

#[derive(Default)]
struct UnboundedSendGroupVisitor {
    extra: Option<Envelope>,
    has_ok: bool,
}

impl UnboundedSendGroupVisitor {
    // We must send while visiting to ensure that a message starting a new actor
    // is actually the first message that the actor receives.
    fn try_send(&mut self, object: &OwnedObject, envelope: Envelope) {
        let actor = object.as_actor().expect("group stores only actors");
        match actor.unbounded_send(envelope) {
            Ok(()) => self.has_ok = true,
            Err(err) => self.extra = Some(err.0),
        }
    }

    fn finish(mut self) -> Result<(), SendError<Envelope>> {
        if self.has_ok {
            Ok(())
        } else {
            let envelope = self.extra.take().expect("missing envelope");
            Err(SendError(envelope))
        }
    }
}

impl GroupVisitor for UnboundedSendGroupVisitor {
    fn done(&mut self) {
        debug_assert!(self.extra.is_none());
        debug_assert!(!self.has_ok);
        self.has_ok = true;
    }

    fn empty(&mut self, envelope: Envelope) {
        debug_assert!(self.extra.is_none());
        debug_assert!(!self.has_ok);
        self.extra = Some(envelope);
    }

    fn visit(&mut self, object: &OwnedObject, envelope: &Envelope) {
        let envelope = self.extra.take().unwrap_or_else(|| envelope.duplicate());
        self.try_send(object, envelope);
    }

    fn visit_last(&mut self, object: &OwnedObject, envelope: Envelope) {
        self.try_send(object, envelope);
    }
}
