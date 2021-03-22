use std::marker::PhantomData;

use crate::{
    addr::Addr,
    address_book::AddressBook,
    demux::Demux,
    envelope::{Envelope, MessageKind},
    errors::{RequestError, SendError, TryRecvError, TrySendError},
    message::{Message, Request},
    request_table::ResponseToken,
};

pub struct Context<C = (), K = ()> {
    addr: Addr,
    book: AddressBook,
    key: K,
    demux: Demux,
    _config: PhantomData<C>,
}

assert_impl_all!(Context: Send);

impl<C, K> Context<C, K> {
    #[inline]
    pub fn addr(&self) -> Addr {
        self.addr
    }

    #[inline]
    pub fn config(&self) -> &C {
        todo!()
    }

    #[inline]
    pub fn key(&self) -> &K {
        &self.key
    }

    pub async fn send<M: Message>(&self, message: M) -> Result<(), SendError<M>> {
        let kind = MessageKind::Regular { sender: self.addr };
        self.do_send(message, kind).await
    }

    #[inline]
    pub fn request<R: Request>(&self, request: R) -> RequestBuilder<'_, C, K, R, Any> {
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
        let message = R::Wrapper::from(message);
        let sender = token.sender;
        let envelope = Envelope::new(message, MessageKind::Regular { sender }).upcast();
        let object = ward!(self.book.get(token.sender));
        let actor = ward!(object.as_actor());
        actor.request_table.respond(token.into_untyped(), envelope);
    }

    #[inline]
    pub async fn recv(&self) -> Option<Envelope> {
        // TODO: cache `OwnedEntry`?
        let object = self.book.get_owned(self.addr)?;
        let actor = object.as_actor()?;
        actor.mailbox.recv().await
    }

    #[inline]
    pub fn try_recv(&self) -> Result<Envelope, TryRecvError> {
        let object = self.book.get(self.addr).ok_or(TryRecvError::Closed)?;
        object
            .as_actor()
            .ok_or(TryRecvError::Closed)?
            .mailbox
            .try_recv()
    }

    pub(crate) fn book(&self) -> &AddressBook {
        &self.book
    }

    pub(crate) fn with_config<C1>(self) -> Context<C1, K> {
        Context {
            addr: self.addr,
            book: self.book,
            key: self.key,
            demux: self.demux,
            _config: PhantomData,
        }
    }

    pub(crate) fn with_addr(mut self, addr: Addr) -> Self {
        self.addr = addr;
        self
    }

    pub(crate) fn with_key<K1>(self, key: K1) -> Context<C, K1> {
        Context {
            addr: self.addr,
            book: self.book,
            key,
            demux: self.demux,
            _config: PhantomData,
        }
    }
}

impl Context<(), ()> {
    pub(crate) fn new(book: AddressBook, demux: Demux) -> Self {
        Self {
            addr: Addr::NULL,
            book,
            key: (),
            demux,
            _config: PhantomData,
        }
    }
}

impl<C, K: Clone> Clone for Context<C, K> {
    fn clone(&self) -> Self {
        Self {
            addr: self.addr,
            book: self.book.clone(),
            key: self.key.clone(),
            _config: PhantomData,
            demux: self.demux.clone(),
        }
    }
}

pub struct RequestBuilder<'c, C, K, R, M> {
    context: &'c Context<C, K>,
    request: R,
    from: Option<Addr>,
    marker: PhantomData<M>,
}

pub struct Any;
pub struct All;

impl<'c, C, K, R> RequestBuilder<'c, C, K, R, Any> {
    fn new(context: &'c Context<C, K>, request: R) -> Self {
        Self {
            context,
            request,
            from: None,
            marker: PhantomData,
        }
    }

    #[inline]
    pub fn all(self) -> RequestBuilder<'c, C, K, R, All> {
        RequestBuilder {
            context: self.context,
            request: self.request,
            from: self.from,
            marker: PhantomData,
        }
    }
}

impl<'c, C, K, R, M> RequestBuilder<'c, C, K, R, M> {
    #[inline]
    pub fn from(mut self, addr: Addr) -> Self {
        self.from = Some(addr);
        self
    }
}

// TODO: add `pub async fn id() { ... }`
impl<'c, C, K, R: Request> RequestBuilder<'c, C, K, R, Any> {
    pub async fn resolve(self) -> Result<R::Response, RequestError<R>> {
        // TODO: cache `OwnedEntry`?
        let this = self.context.addr;
        let object = self.context.book.get_owned(this).expect("invalid addr");
        let actor = object.as_actor().expect("can be called only on actors");
        let token = actor.request_table.new_request();
        let request_id = token.request_id;
        let message_kind = MessageKind::RequestAny(token);

        if let Some(recipient) = self.from {
            let rec_entry = self.context.book.get_owned(recipient);
            let rec_object = ward!(rec_entry, return Err(RequestError::Closed(self.request)));
            let envelope = Envelope::new(self.request, message_kind);
            let result = rec_object.send(self.context, envelope.upcast()).await;
            result.map_err(|err| RequestError::Closed(err.0.do_downcast().into_message()))?;
        } else {
            let fut = self.context.do_send(self.request, message_kind).await;
            fut.map_err(|err| RequestError::Closed(err.0))?;
        }

        let mut data = actor.request_table.wait(request_id).await;
        if let Some(Some(envelope)) = data.pop() {
            Ok(envelope.do_downcast::<R::Wrapper>().into_message().into())
        } else {
            Err(RequestError::Ignored)
        }
    }
}

impl<'c, C, K, R: Request> RequestBuilder<'c, C, K, R, All> {
    pub async fn resolve(self) -> Vec<Result<R::Response, RequestError<R>>> {
        // TODO: cache `OwnedEntry`?
        let this = self.context.addr;
        let object = self.context.book.get_owned(this).expect("invalid addr");
        let actor = object.as_actor().expect("can be called only on actors");
        let token = actor.request_table.new_request();
        let request_id = token.request_id;
        let message_kind = MessageKind::RequestAll(token);

        if let Some(recipient) = self.from {
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
        } else if let Err(err) = self.context.do_send(self.request, message_kind).await {
            return vec![Err(RequestError::Closed(err.0))];
        }

        actor
            .request_table
            .wait(request_id)
            .await
            .into_iter()
            .map(|opt| match opt {
                Some(envelope) => Ok(envelope.do_downcast::<R::Wrapper>().into_message().into()),
                None => Err(RequestError::Ignored),
            })
            .collect()
    }
}
