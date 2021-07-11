use std::{marker::PhantomData, sync::Arc};

use futures::{
    future::{poll_fn, FutureExt},
    pin_mut, select_biased,
};
use tracing::{info, trace};

use crate::{self as elfo};
use elfo_macros::msg_raw as msg;

use crate::{
    actor::ActorStatus,
    addr::Addr,
    address_book::AddressBook,
    config::AnyConfig,
    demux::Demux,
    dumping::Dumper,
    envelope::{Envelope, MessageKind},
    errors::{RequestError, SendError, TryRecvError, TrySendError},
    message::{Message, Request},
    messages,
    request_table::ResponseToken,
    routers::Singleton,
    tls,
};

use self::source::Combined;
pub(crate) use self::source::Source;

mod source;

pub struct Context<C = (), K = Singleton, S = ()> {
    book: AddressBook,
    dumper: Dumper,
    addr: Addr,
    group: Addr,
    demux: Demux,
    config: Arc<C>,
    key: K,
    source: S,
}

assert_impl_all!(Context: Send);

impl<C, K, S> Context<C, K, S> {
    #[inline]
    pub fn addr(&self) -> Addr {
        self.addr
    }

    #[inline]
    pub fn group(&self) -> Addr {
        self.group
    }

    #[deprecated]
    #[cfg(feature = "test-util")]
    #[cfg_attr(docsrs, doc(cfg(feature = "test-util")))]
    pub fn set_addr(&mut self, addr: Addr) {
        self.addr = addr;
    }

    #[inline]
    pub fn config(&self) -> &C {
        &self.config
    }

    #[inline]
    pub fn key(&self) -> &K {
        &self.key
    }

    pub fn with<S1>(self, source: S1) -> Context<C, K, Combined<S, S1>> {
        Context {
            book: self.book,
            dumper: self.dumper,
            addr: self.addr,
            group: self.group,
            demux: self.demux,
            config: self.config,
            key: self.key,
            source: Combined::new(self.source, source),
        }
    }

    pub fn set_status(&self, status: ActorStatus) {
        let object = ward!(self.book.get_owned(self.addr));
        let actor = ward!(object.as_actor());
        actor.set_status(status);
    }

    pub async fn send<M: Message>(&self, message: M) -> Result<(), SendError<M>> {
        let kind = MessageKind::Regular { sender: self.addr };
        trace!("> {:?}", message);
        self.do_send(message, kind).await
    }

    #[inline]
    pub fn request<R: Request>(&self, request: R) -> RequestBuilder<'_, C, K, S, R, Any> {
        RequestBuilder::new(self, request)
    }

    async fn do_send<M: Message>(&self, message: M, kind: MessageKind) -> Result<(), SendError<M>> {
        let envelope = Envelope::new(message, kind).upcast();
        let addrs = self.demux.filter(&envelope);

        if addrs.is_empty() {
            return Err(SendError(envelope.do_downcast().into_message()));
        }

        if addrs.len() == 1 {
            return match self.book.get_owned(addrs[0]) {
                Some(object) => object
                    .send(self, envelope)
                    .await
                    .map_err(|err| SendError(err.0.do_downcast().into_message())),
                None => Err(SendError(envelope.do_downcast().into_message())),
            };
        }

        let mut unused = None;
        let mut success = false;

        // TODO: use the visitor pattern in order to avoid extra cloning.
        // TODO: send concurrently.
        for addr in addrs {
            let envelope = unused.take().or_else(|| envelope.duplicate(&self.book));
            let envelope = ward!(envelope, break);

            match self.book.get_owned(addr) {
                Some(object) => {
                    unused = object.send(self, envelope).await.err().map(|err| err.0);
                    if unused.is_none() {
                        success = true;
                    }
                }
                None => unused = Some(envelope),
            };
        }

        if success {
            Ok(())
        } else {
            Err(SendError(envelope.do_downcast().into_message()))
        }
    }

    pub async fn send_to<M: Message>(
        &self,
        recipient: Addr,
        message: M,
    ) -> Result<(), SendError<M>> {
        let entry = self.book.get_owned(recipient);
        let object = ward!(entry, return Err(SendError(message)));
        trace!(to = %recipient, "> {:?}", message);
        let envelope = Envelope::new(message, MessageKind::Regular { sender: self.addr });
        let fut = object.send(self, envelope.upcast());
        let result = fut.await;
        result.map_err(|err| SendError(err.0.do_downcast().into_message()))
    }

    pub fn try_send_to<M: Message>(
        &self,
        recipient: Addr,
        message: M,
    ) -> Result<(), TrySendError<M>> {
        let entry = self.book.get_owned(recipient);
        let object = ward!(entry, return Err(TrySendError::Closed(message)));
        trace!(to = %recipient, "> {:?}", message);
        let envelope = Envelope::new(message, MessageKind::Regular { sender: self.addr });

        object.try_send(envelope.upcast()).map_err(|err| match err {
            TrySendError::Full(envelope) => {
                TrySendError::Full(envelope.do_downcast().into_message())
            }
            TrySendError::Closed(envelope) => {
                TrySendError::Closed(envelope.do_downcast().into_message())
            }
        })
    }

    pub fn respond<R: Request>(&self, token: ResponseToken<R>, message: R::Response) {
        if token.is_forgotten() {
            return;
        }

        let message = R::Wrapper::from(message);
        let sender = token.sender;
        trace!(to = %sender, "> {:?}", message);
        let envelope = Envelope::new(message, MessageKind::Regular { sender }).upcast();
        let object = ward!(self.book.get(token.sender));
        let actor = ward!(object.as_actor());
        actor
            .request_table()
            .respond(token.into_untyped(), envelope);
    }

    #[inline]
    pub async fn recv(&mut self) -> Option<Envelope>
    where
        C: 'static,
        S: Source,
    {
        // TODO: cache `OwnedEntry`?
        let object = self.book.get_owned(self.addr)?;
        let actor = object.as_actor()?;

        // TODO: remove `fuse`.
        let mailbox_fut = actor.recv().fuse();
        let source_fut = poll_fn(|cx| self.source.poll_recv(cx)).fuse();

        pin_mut!(mailbox_fut);
        pin_mut!(source_fut);

        // TODO: reset trace_id for these logs?
        let envelope = select_biased! {
            envelope = mailbox_fut => match envelope {
                Some(envelope) => envelope,
                None => {
                    trace!("mailbox closed");
                    return None;
                }
            },
            envelope = source_fut => match envelope {
                Some(envelope) => envelope,
                None => {
                    // TODO: rerun select?
                    trace!("some of sources was closed");
                    return None;
                }
            },
        };

        tls::set_trace_id(envelope.trace_id());

        let envelope = msg!(match envelope {
            (messages::UpdateConfig { config }, token) => {
                self.config = config.get().cloned().expect("must be decoded");
                info!("config updated");
                let message = messages::ConfigUpdated {};
                let kind = MessageKind::Regular { sender: self.addr };
                let envelope = Envelope::new(message, kind).upcast();
                self.respond(token, Ok(()));
                envelope
            }
            envelope => envelope,
        });

        trace!("< {:?}", envelope.message());

        Some(envelope)
    }

    #[inline]
    pub fn try_recv(&mut self) -> Result<Envelope, TryRecvError>
    where
        C: 'static,
    {
        let object = self.book.get(self.addr).ok_or(TryRecvError::Closed)?;
        let actor = object.as_actor().ok_or(TryRecvError::Closed)?;
        let envelope = match actor.try_recv() {
            Ok(envelope) => envelope,
            Err(err) => {
                if err.is_closed() {
                    // TODO: reset trace_id for this log?
                    trace!("mailbox closed");
                }
                return Err(err);
            }
        };

        tls::set_trace_id(envelope.trace_id());

        // TODO: poll the sources.

        let envelope = msg!(match envelope {
            (messages::UpdateConfig { config }, token) => {
                self.config = config.get().cloned().expect("must be decoded");
                info!("config updated");
                let message = messages::ConfigUpdated {};
                let kind = MessageKind::Regular { sender: self.addr };
                let envelope = Envelope::new(message, kind).upcast();
                self.respond(token, Ok(()));
                envelope
            }
            envelope => envelope,
        });

        trace!("< {:?}", envelope.message());

        Ok(envelope)
    }

    /// XXX: mb `BoundEnvelope<C>`?
    pub fn unpack_config<'c>(&self, config: &'c AnyConfig) -> &'c C
    where
        C: for<'de> serde::Deserialize<'de> + 'static,
    {
        config.get().expect("must already be checked")
    }

    pub fn pruned(&self) -> Context {
        Context {
            book: self.book.clone(),
            dumper: self.dumper.clone(),
            addr: self.addr,
            group: self.group,
            demux: self.demux.clone(),
            config: Arc::new(()),
            key: Singleton,
            source: (),
        }
    }

    pub(crate) fn book(&self) -> &AddressBook {
        &self.book
    }

    pub(crate) fn dumper(&self) -> &Dumper {
        &self.dumper
    }

    pub(crate) fn with_config<C1>(self, config: Arc<C1>) -> Context<C1, K, S> {
        Context {
            book: self.book,
            dumper: self.dumper,
            addr: self.addr,
            group: self.group,
            demux: self.demux,
            config,
            key: self.key,
            source: self.source,
        }
    }

    pub(crate) fn with_addr(mut self, addr: Addr) -> Self {
        self.addr = addr;
        self
    }

    pub(crate) fn with_group(mut self, group: Addr) -> Self {
        self.group = group;
        self
    }

    pub(crate) fn with_key<K1>(self, key: K1) -> Context<C, K1, S> {
        Context {
            book: self.book,
            dumper: self.dumper,
            addr: self.addr,
            group: self.group,
            demux: self.demux,
            config: self.config,
            key,
            source: self.source,
        }
    }
}

impl Context {
    pub(crate) fn new(book: AddressBook, dumper: Dumper, demux: Demux) -> Self {
        Self {
            book,
            dumper,
            addr: Addr::NULL,
            group: Addr::NULL,
            demux,
            config: Arc::new(()),
            key: Singleton,
            source: (),
        }
    }
}

impl<C, K: Clone> Clone for Context<C, K> {
    fn clone(&self) -> Self {
        Self {
            book: self.book.clone(),
            dumper: self.dumper.clone(),
            addr: self.addr,
            group: self.group,
            demux: self.demux.clone(),
            config: self.config.clone(),
            key: self.key.clone(),
            source: (),
        }
    }
}

#[must_use]
pub struct RequestBuilder<'c, C, K, S, R, M> {
    context: &'c Context<C, K, S>,
    request: R,
    from: Option<Addr>,
    marker: PhantomData<M>,
}

pub struct Any;
pub struct All;
pub(crate) struct Forgotten;

impl<'c, C, K, S, R> RequestBuilder<'c, C, K, S, R, Any> {
    fn new(context: &'c Context<C, K, S>, request: R) -> Self {
        Self {
            context,
            request,
            from: None,
            marker: PhantomData,
        }
    }

    #[inline]
    pub fn all(self) -> RequestBuilder<'c, C, K, S, R, All> {
        RequestBuilder {
            context: self.context,
            request: self.request,
            from: self.from,
            marker: PhantomData,
        }
    }

    // TODO
    #[allow(unused)]
    pub(crate) fn forgotten(self) -> RequestBuilder<'c, C, K, S, R, Forgotten> {
        RequestBuilder {
            context: self.context,
            request: self.request,
            from: self.from,
            marker: PhantomData,
        }
    }
}

impl<'c, C, K, S, R, M> RequestBuilder<'c, C, K, S, R, M> {
    #[inline]
    pub fn from(mut self, addr: Addr) -> Self {
        self.from = Some(addr);
        self
    }
}

// TODO: add `pub async fn id() { ... }`
impl<'c, C: 'static, K, S, R: Request> RequestBuilder<'c, C, S, K, R, Any> {
    pub async fn resolve(self) -> Result<R::Response, RequestError<R>> {
        // TODO: cache `OwnedEntry`?
        let this = self.context.addr;
        let object = self.context.book.get_owned(this).expect("invalid addr");
        let actor = object.as_actor().expect("can be called only on actors");
        let token = actor
            .request_table()
            .new_request(self.context.book.clone(), false);
        let request_id = token.request_id;
        let message_kind = MessageKind::RequestAny(token);

        if let Some(recipient) = self.from {
            trace!(message = ?self.request, to = %recipient, ">");
            let rec_entry = self.context.book.get_owned(recipient);
            let rec_object = ward!(rec_entry, return Err(RequestError::Closed(self.request)));
            let envelope = Envelope::new(self.request, message_kind);
            let result = rec_object.send(self.context, envelope.upcast()).await;
            result.map_err(|err| RequestError::Closed(err.0.do_downcast().into_message()))?;
        } else {
            trace!(message = ?self.request, ">");
            let fut = self.context.do_send(self.request, message_kind).await;
            fut.map_err(|err| RequestError::Closed(err.0))?;
        }

        let mut data = actor.request_table().wait(request_id).await;
        if let Some(Some(envelope)) = data.pop() {
            let message = envelope.do_downcast::<R::Wrapper>().into_message().into();
            trace!(?message, "<");
            Ok(message)
        } else {
            Err(RequestError::Ignored)
        }
    }
}

impl<'c, C: 'static, K, S, R: Request> RequestBuilder<'c, C, K, S, R, All> {
    pub async fn resolve(self) -> Vec<Result<R::Response, RequestError<R>>> {
        // TODO: cache `OwnedEntry`?
        let this = self.context.addr;
        let object = self.context.book.get_owned(this).expect("invalid addr");
        let actor = object.as_actor().expect("can be called only on actors");
        let token = actor
            .request_table()
            .new_request(self.context.book.clone(), true);
        let request_id = token.request_id;
        let message_kind = MessageKind::RequestAll(token);

        if let Some(recipient) = self.from {
            trace!(message = ?self.request, to = %recipient, ">");
            let rec_entry = self.context.book.get_owned(recipient);
            let rec_object = ward!(
                rec_entry,
                return vec![Err(RequestError::Closed(self.request))]
            );
            let envelope = Envelope::new(self.request, message_kind);
            if let Err(err) = rec_object.send(self.context, envelope.upcast()).await {
                let msg = err.0.do_downcast().into_message();
                return vec![Err(RequestError::Closed(msg))];
            }
        } else {
            trace!(message = ?self.request, ">");

            if let Err(err) = self.context.do_send(self.request, message_kind).await {
                return vec![Err(RequestError::Closed(err.0))];
            }
        }

        actor
            .request_table()
            .wait(request_id)
            .await
            .into_iter()
            .map(|opt| match opt {
                Some(envelope) => Ok(envelope.do_downcast::<R::Wrapper>().into_message().into()),
                None => Err(RequestError::Ignored),
            })
            .inspect(|res| {
                if let Ok(message) = res {
                    trace!(?message, "<");
                }
            })
            .collect()
    }
}

impl<'c, C: 'static, K, S, R: Request> RequestBuilder<'c, C, S, K, R, Forgotten> {
    pub async fn resolve(self) -> Result<R::Response, RequestError<R>> {
        let token = ResponseToken::forgotten(self.context.book.clone());
        let message_kind = MessageKind::RequestAny(token);

        if let Some(recipient) = self.from {
            trace!(message = ?self.request, to = %recipient, ">");
            let rec_entry = self.context.book.get_owned(recipient);
            let rec_object = ward!(rec_entry, return Err(RequestError::Closed(self.request)));
            let envelope = Envelope::new(self.request, message_kind);
            let result = rec_object.send(self.context, envelope.upcast()).await;
            result.map_err(|err| RequestError::Closed(err.0.do_downcast().into_message()))?;
        } else {
            trace!(message = ?self.request, ">");
            let fut = self.context.do_send(self.request, message_kind).await;
            fut.map_err(|err| RequestError::Closed(err.0))?;
        }

        Err(RequestError::Ignored)
    }
}
