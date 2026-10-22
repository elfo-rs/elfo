use std::{
    future::Future,
    mem,
    ops::Deref,
    pin::Pin,
    ptr::NonNull,
    task::{self, Poll},
};

use derive_more::From;
use futures::future::{BoxFuture, join_all};
use idr_ebr::{BorrowedEntry, OwnedEntry};
use pin_project::pin_project;
use smallvec::SmallVec;

#[cfg(feature = "network")]
use crate::remote::{self, RemoteHandle};
use crate::{
    actor::Actor,
    addr::Addr,
    envelope::{Envelope, RejectedEnvelope},
    errors::{SendError, TrySendError},
    request_table::{RequestFailure, ResponseToken},
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

// === MappedOwnedObject ===

/// An [`OwnedObject`] projected to one of its fields.
pub(crate) struct MappedOwnedObject<P> {
    projected: NonNull<P>,
    entry: OwnedObject,
}

// SAFETY: `Object: Sync` is asserted above; `map`'s forces `P` to be borrowed
// from `&Object`, so `P` is transitively `Send + Sync`.
unsafe impl<P> Send for MappedOwnedObject<P> {}
// SAFETY: Same as above.
unsafe impl<P> Sync for MappedOwnedObject<P> {}

impl<P> MappedOwnedObject<P> {
    pub(crate) fn map<F>(entry: OwnedObject, proj: F) -> Self
    where
        F: FnOnce(&Object) -> &P,
    {
        let projected = NonNull::from(proj(&entry));
        Self { entry, projected }
    }
}

impl<P> Deref for MappedOwnedObject<P> {
    type Target = P;

    #[inline]
    fn deref(&self) -> &P {
        // SAFETY: `entry` keeps the object alive and `idr_ebr::OwnedEntry`
        // pins it at a stable heap address, so `projected` remains valid for the
        // lifetime of `entry`.
        unsafe { self.projected.as_ref() }
    }
}

impl<P> Clone for MappedOwnedObject<P> {
    fn clone(&self) -> Self {
        Self {
            entry: self.entry.clone(),
            projected: self.projected,
        }
    }
}

#[derive(From)]
#[allow(clippy::large_enum_variant)]
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

    #[instability::unstable]
    #[inline]
    pub fn addr(&self) -> Addr {
        self.addr
    }

    /// Tries to send an envelope synchronously and returns its result or continuation.
    ///
    /// If an object is full, acquires an owned link to it for the continuation,
    /// because an EBR guard cannot be held across an async boundary.
    #[instability::unstable]
    pub fn send(
        this: ObjectRef<'_>,
        recipient: Addr,
        envelope: Envelope,
    ) -> SendFut<impl Future<Output = SendOutput> + use<>> {
        let _ = recipient; // suppress a warning if the "network" feature is disabled

        match &this.kind {
            ObjectKind::Actor(handle) => match handle.try_send(envelope) {
                Ok(()) => SendFut::Ready(Ok(())),
                Err(TrySendError::Closed(envelope)) => {
                    SendFut::Ready(Err(SendError(envelope.reject(this.addr))))
                }
                Err(TrySendError::Full(envelope)) => {
                    let Some(this) = this.to_owned() else {
                        return SendFut::Ready(Err(SendError(envelope.reject(this.addr))));
                    };

                    SendFut::Pending(WaitFut::Actor(async move {
                        let actor = this.as_actor().unwrap();
                        actor
                            .send(envelope)
                            .await
                            .map_err(|err| err.map(|envelope| envelope.reject(this.addr)))
                    }))
                }
            },
            ObjectKind::Group(handle) => {
                let mut visitor = SendGroupVisitor::default();
                handle.handle(envelope, &mut visitor);
                if visitor.is_ready() {
                    SendFut::Ready(visitor.finish())
                } else {
                    SendFut::Pending(WaitFut::Group(visitor.wait()))
                }
            }
            #[cfg(feature = "network")]
            ObjectKind::Remote(handle) => match handle.try_send(recipient, envelope) {
                Ok(()) => SendFut::Ready(Ok(())),
                Err(TrySendError::Closed(envelope)) => {
                    SendFut::Ready(Err(SendError(envelope.reject(Addr::NULL))))
                }
                Err(TrySendError::Full(mut envelope)) => {
                    let Some(this) = this.to_owned() else {
                        return SendFut::Ready(Err(SendError(envelope.reject(Addr::NULL))));
                    };

                    SendFut::Pending(WaitFut::Remote(async move {
                        let handle = this.as_remote().unwrap();
                        loop {
                            match handle.send(recipient, envelope) {
                                remote::SendResult::Ok => break Ok(()),
                                remote::SendResult::Err(err) => {
                                    break Err(err.map(|envelope| envelope.reject(Addr::NULL)));
                                }
                                remote::SendResult::Wait(notified, e) => {
                                    envelope = e;
                                    notified.await;
                                }
                            }
                        }
                    }))
                }
            },
        }
    }

    #[instability::unstable]
    pub fn try_send(
        &self,
        recipient: Addr,
        envelope: Envelope,
    ) -> Result<(), TrySendError<RejectedEnvelope>> {
        let _ = recipient; // suppress a warning if the "network" feature is disabled

        match &self.kind {
            ObjectKind::Actor(handle) => handle
                .try_send(envelope)
                .map_err(|err| err.map(|envelope| envelope.reject(self.addr))),
            ObjectKind::Group(handle) => {
                let mut visitor = TrySendGroupVisitor::default();
                handle.handle(envelope, &mut visitor);
                visitor.finish()
            }
            #[cfg(feature = "network")]
            ObjectKind::Remote(handle) => handle
                .try_send(recipient, envelope)
                .map_err(|err| err.map(|envelope| envelope.reject(Addr::NULL))),
        }
    }

    #[instability::unstable]
    pub fn unbounded_send(
        &self,
        recipient: Addr,
        envelope: Envelope,
    ) -> Result<(), SendError<RejectedEnvelope>> {
        let _ = recipient; // suppress a warning if the "network" feature is disabled

        match &self.kind {
            ObjectKind::Actor(handle) => handle
                .unbounded_send(envelope)
                .map_err(|err| err.map(|envelope| envelope.reject(self.addr))),
            ObjectKind::Group(handle) => {
                let mut visitor = UnboundedSendGroupVisitor::default();
                handle.handle(envelope, &mut visitor);
                visitor.finish()
            }
            #[cfg(feature = "network")]
            ObjectKind::Remote(handle) => handle
                .unbounded_send(recipient, envelope)
                .map_err(|err| err.map(|envelope| envelope.reject(Addr::NULL))),
        }
    }

    #[instability::unstable]
    pub fn respond(&self, token: ResponseToken, response: Result<Envelope, RequestFailure>) {
        match &self.kind {
            ObjectKind::Actor(handle) => handle.request_table().resolve(token, response),
            ObjectKind::Group(_handle) => unreachable!(),
            #[cfg(feature = "network")]
            ObjectKind::Remote(handle) => handle.respond(token, response),
        }
    }

    #[instability::unstable]
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

// === ObjectRef ===

/// Access to an object through an EBR guard or an existing owned handle.
// Reexported in `_priv`.
#[doc(hidden)]
#[instability::unstable]
#[derive(From)]
pub enum ObjectRef<'a> {
    Borrowed(BorrowedObject<'a>),
    Owned(&'a OwnedObject),
}

impl ObjectRef<'_> {
    #[inline]
    fn to_owned(&self) -> Option<OwnedObject> {
        match self {
            Self::Borrowed(object) => (*object).to_owned(),
            Self::Owned(object) => Some((*object).clone()),
        }
    }
}

impl Deref for ObjectRef<'_> {
    type Target = Object;

    #[inline]
    fn deref(&self) -> &Object {
        match self {
            Self::Borrowed(object) => object,
            Self::Owned(object) => object,
        }
    }
}

// === SendFut ===

type SendOutput = Result<(), SendError<RejectedEnvelope>>;

/// The completed result of [Object::send] or the work needed to finish it.
// Reexported in `_priv`.
#[doc(hidden)]
#[instability::unstable]
#[must_use]
#[pin_project(project = SendFutProj)]
pub enum SendFut<F> {
    Ready(SendOutput),
    Pending(#[pin] F),
}

impl<F: Future<Output = SendOutput>> Future for SendFut<F> {
    type Output = SendOutput;

    #[inline]
    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<Self::Output> {
        match self.project() {
            SendFutProj::Ready(result) => Poll::Ready(mem::replace(result, Ok(()))),
            SendFutProj::Pending(future) => future.poll(cx),
        }
    }
}

// === WaitFut ===

#[cfg(not(feature = "network"))]
#[pin_project(project = WaitFutProj)]
enum WaitFut<A, G> {
    Actor(#[pin] A),
    Group(#[pin] G),
}

#[cfg(not(feature = "network"))]
impl<A, G> Future for WaitFut<A, G>
where
    A: Future<Output = SendOutput>,
    G: Future<Output = SendOutput>,
{
    type Output = SendOutput;

    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<Self::Output> {
        match self.project() {
            WaitFutProj::Actor(fut) => fut.poll(cx),
            WaitFutProj::Group(fut) => fut.poll(cx),
        }
    }
}

#[cfg(feature = "network")]
#[pin_project(project = WaitFutProj)]
enum WaitFut<A, G, R> {
    Actor(#[pin] A),
    Group(#[pin] G),
    Remote(#[pin] R),
}

#[cfg(feature = "network")]
impl<A, G, R> Future for WaitFut<A, G, R>
where
    A: Future<Output = SendOutput>,
    G: Future<Output = SendOutput>,
    R: Future<Output = SendOutput>,
{
    type Output = SendOutput;

    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<Self::Output> {
        match self.project() {
            WaitFutProj::Actor(fut) => fut.poll(cx),
            WaitFutProj::Group(fut) => fut.poll(cx),
            WaitFutProj::Remote(fut) => fut.poll(cx),
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
    /// The last rejected envelope, returned to the caller if nobody accepted.
    extra: Option<RejectedEnvelope>,
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
                self.extra = Some(envelope.reject(object.addr));
            }
        }
    }

    #[inline]
    fn is_ready(&self) -> bool {
        self.full.is_empty()
    }

    #[inline]
    async fn wait(mut self) -> SendOutput {
        // Wait until messages reach all full actors.
        #[allow(clippy::comparison_chain)]
        if self.full.len() == 1 {
            let (object, envelope) = self.full.pop().unwrap();

            let actor = object.as_actor().expect("group stores only actors");
            match actor.send(envelope).await {
                Ok(()) => self.has_ok = true,
                Err(SendError(envelope)) => {
                    let rejected = envelope.reject(object.addr);
                    if !self.has_ok {
                        self.extra = Some(rejected);
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
                        .map_err(|err| err.map(|envelope| envelope.reject(object.addr)))
                });
            }

            for result in join_all(futures).await {
                match result {
                    Ok(()) => self.has_ok = true,
                    Err(SendError(rejected)) => {
                        if !self.has_ok {
                            self.extra = Some(rejected);
                        }
                    }
                }
            }
        }

        self.finish()
    }

    fn finish(mut self) -> SendOutput {
        debug_assert!(self.is_ready());

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
        self.extra = Some(envelope.reject(Addr::NULL));
    }

    fn visit(&mut self, object: &OwnedObject, envelope: &Envelope) {
        let envelope = match self.extra.take() {
            Some(rejected) => reuse_envelope(rejected, envelope),
            None => envelope.duplicate(),
        };
        self.try_send(object, envelope);
    }

    fn visit_last(&mut self, object: &OwnedObject, envelope: Envelope) {
        self.try_send(object, envelope);
    }
}

// === TrySendGroupVisitor ===

#[derive(Default)]
struct TrySendGroupVisitor {
    extra: Option<RejectedEnvelope>,
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
                self.extra = Some(err.into_inner().reject(object.addr));
            }
        }
    }

    fn finish(mut self) -> Result<(), TrySendError<RejectedEnvelope>> {
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
        self.extra = Some(envelope.reject(Addr::NULL));
    }

    fn visit(&mut self, object: &OwnedObject, envelope: &Envelope) {
        let envelope = match self.extra.take() {
            Some(rejected) => reuse_envelope(rejected, envelope),
            None => envelope.duplicate(),
        };
        self.try_send(object, envelope);
    }

    fn visit_last(&mut self, object: &OwnedObject, envelope: Envelope) {
        self.try_send(object, envelope);
    }
}

// === UnboundedSendGroupVisitor ===

#[derive(Default)]
struct UnboundedSendGroupVisitor {
    extra: Option<RejectedEnvelope>,
    has_ok: bool,
}

impl UnboundedSendGroupVisitor {
    // We must send while visiting to ensure that a message starting a new actor
    // is actually the first message that the actor receives.
    fn try_send(&mut self, object: &OwnedObject, envelope: Envelope) {
        let actor = object.as_actor().expect("group stores only actors");
        match actor.unbounded_send(envelope) {
            Ok(()) => self.has_ok = true,
            Err(err) => self.extra = Some(err.into_inner().reject(object.addr)),
        }
    }

    fn finish(mut self) -> Result<(), SendError<RejectedEnvelope>> {
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
        self.extra = Some(envelope.reject(Addr::NULL));
    }

    fn visit(&mut self, object: &OwnedObject, envelope: &Envelope) {
        let envelope = match self.extra.take() {
            Some(rejected) => reuse_envelope(rejected, envelope),
            None => envelope.duplicate(),
        };
        self.try_send(object, envelope);
    }

    fn visit_last(&mut self, object: &OwnedObject, envelope: Envelope) {
        self.try_send(object, envelope);
    }
}

// The rejected envelope is a duplicate of `source`.
fn reuse_envelope(rejected: RejectedEnvelope, source: &Envelope) -> Envelope {
    let mut envelope = rejected.into_inner();
    debug_assert_eq!(envelope.type_id(), source.type_id());
    if let (Some(token), Some(source)) = (envelope.token_mut(), source.token()) {
        debug_assert!(token.is_forgotten());
        *token = source.duplicate();
    }
    envelope
}
