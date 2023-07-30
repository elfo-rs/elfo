use derive_more::From;
use futures::future::{join_all, BoxFuture};
use smallvec::SmallVec;

#[cfg(feature = "network")]
use crate::remote::{self, RemoteHandle};
use crate::{
    actor::Actor,
    address_book::{AddressBook, SlabConfig},
    context::Context,
    envelope::Envelope,
    errors::{RequestError, SendError, TrySendError},
    request_table::ResponseToken,
    Addr,
};

// Reexported in `_priv`.
pub struct Object {
    addr: Addr,
    kind: ObjectKind,
}

assert_impl_all!(Object: Sync);
// TODO: actually, `Slab::Entry<Object>` should be aligned.
// assert_eq_size!(Object, [u8; 256]);

// TODO: move to `address_book` and wrap to avoid calling the `key()` method.
pub(crate) type ObjectRef<'a> = sharded_slab::Entry<'a, Object, SlabConfig>;
// Reexported in `_priv`.
pub type ObjectArc = sharded_slab::OwnedEntry<Object, SlabConfig>;

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
    pub fn addr(&self) -> Addr {
        self.addr
    }

    // TODO: pass `&mut Option<(Addr, Envelope)>` to avoid extra moves.
    #[stability::unstable]
    pub async fn send<C, K>(
        &self,
        ctx: &Context<C, K>,
        recipient: Addr,
        envelope: Envelope,
    ) -> Result<(), SendError<Envelope>> {
        match &self.kind {
            ObjectKind::Actor(handle) => handle.send(envelope).await,
            ObjectKind::Group(handle) => {
                let mut visitor = SendGroupVisitor::new(ctx.book());
                handle.handle(envelope, &mut visitor);
                visitor.finish().await
            }
            #[cfg(feature = "network")]
            ObjectKind::Remote(handle) => {
                let mut envelope = envelope;
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
            }
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
            ObjectKind::Actor(actor) => Some(actor),
            ObjectKind::Group(_) => None,
            #[cfg(feature = "network")]
            ObjectKind::Remote(_) => None,
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
    fn visit(&mut self, object: &ObjectArc, envelope: &Envelope);
    fn visit_last(&mut self, object: &ObjectArc, envelope: Envelope);
}

// === SendGroupVisitor ===

struct SendGroupVisitor<'a> {
    book: &'a AddressBook,
    full: SmallVec<[(Addr, Envelope); 1]>,
    extra: Option<Envelope>,
    has_ok: bool,
}

impl<'a> SendGroupVisitor<'a> {
    fn new(book: &'a AddressBook) -> Self {
        Self {
            book,
            full: Default::default(),
            extra: None,
            has_ok: false,
        }
    }

    // We must send while visiting to ensure that a message starting a new actor
    // is actually the first message that the actor receives.
    fn try_send(&mut self, object: &ObjectArc, envelope: Envelope) {
        let actor = object.as_actor().expect("group stores only actors");
        match actor.try_send(envelope) {
            Ok(()) => self.has_ok = true,
            Err(TrySendError::Full(envelope)) => {
                self.full.push((object.addr(), envelope));
            }
            Err(TrySendError::Closed(envelope)) => {
                self.extra = Some(envelope);
            }
        }
    }

    #[inline]
    async fn finish(mut self) -> Result<(), SendError<Envelope>> {
        // Wait until messages reach all full actors.
        #[allow(clippy::comparison_chain)]
        if self.full.len() == 1 {
            let (addr, envelope) = self.full.pop().unwrap();

            if let Some(object) = self.book.get_owned(addr) {
                let actor = object.as_actor().expect("group stores only actors");
                match actor.send(envelope).await {
                    Ok(()) => self.has_ok = true,
                    Err(SendError(envelope)) => {
                        if !self.has_ok {
                            self.extra = Some(envelope);
                        }
                    }
                }
            } else if !self.has_ok {
                self.extra = Some(envelope);
            }
        } else if self.full.len() > 1 {
            let mut futures = Vec::new();

            for (addr, envelope) in self.full.drain(..) {
                let object = self.book.get_owned(addr);
                futures.push(async move {
                    match object {
                        Some(object) => {
                            object
                                .as_actor()
                                .expect("group stores only actors")
                                .send(envelope)
                                .await
                        }
                        None => Err(SendError(envelope)),
                    }
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

impl GroupVisitor for SendGroupVisitor<'_> {
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

    fn visit(&mut self, object: &ObjectArc, envelope: &Envelope) {
        let envelope = self.extra.take().unwrap_or_else(|| envelope.duplicate());
        self.try_send(object, envelope);
    }

    fn visit_last(&mut self, object: &ObjectArc, envelope: Envelope) {
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
    fn try_send(&mut self, object: &ObjectArc, envelope: Envelope) {
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

    fn visit(&mut self, object: &ObjectArc, envelope: &Envelope) {
        let envelope = self.extra.take().unwrap_or_else(|| envelope.duplicate());
        self.try_send(object, envelope);
    }

    fn visit_last(&mut self, object: &ObjectArc, envelope: Envelope) {
        self.try_send(object, envelope);
    }
}
