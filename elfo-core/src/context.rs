use std::marker::PhantomData;

use crate::{
    addr::Addr,
    address_book::AddressBook,
    demux::Demux,
    envelope::{Envelope, MessageKind, ReplyToken},
    mailbox::{SendError, TryRecvError},
    message::{Message, Request},
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
        self.do_send(message, MessageKind::regular()).await
    }

    pub async fn request<R: Request>(
        &self,
        request: R,
        // TODO: avoid `Option`.
    ) -> Result<R::Response, SendError<Option<R>>> {
        let (rx, kind) = MessageKind::request();
        self.do_send(request, kind)
            .await
            .map_err(|err| SendError(Some(err.0)))?;
        let envelope = rx.receive().await.ok_or(SendError(None))?;
        Ok(envelope
            .downcast()
            .expect("invalid response")
            .into_message())
    }

    async fn do_send<M: Message>(&self, message: M, kind: MessageKind) -> Result<(), SendError<M>> {
        let envelope = Envelope::new(self.addr, message, kind).upcast();
        let addrs = self.demux.filter(&envelope);

        if addrs.is_empty() {
            return Err(SendError(downcast(envelope)));
        }

        let mut unused = None;
        let mut success = false;

        // TODO: use the visitor pattern in order to avoid extra cloning.
        // TODO: send concurrently.
        for addr in addrs {
            // TODO: `clone` for requests is broken.
            let envelope = unused.take().unwrap_or_else(|| envelope.clone());

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
            Err(SendError(downcast(envelope)))
        }
    }

    pub async fn send_to<M: Message>(
        &self,
        recipient: Addr,
        message: M,
    ) -> Result<(), SendError<M>> {
        let entry = self.book.get_owned(recipient);
        let object = ward!(entry, return Err(SendError(message)));
        let envelope = Envelope::new(self.addr, message, MessageKind::regular());
        let fut = object.send(self, envelope.upcast());
        let result = fut.await;
        result.map_err(|err| SendError(err.0.downcast().expect("impossible").into_message()))
    }

    pub async fn request_from<R: Request>(
        &self,
        recipient: Addr,
        message: R,
        // TODO: avoid `Option`.
    ) -> Result<R::Response, SendError<Option<R>>> {
        let entry = self.book.get_owned(recipient);
        let object = ward!(entry, return Err(SendError(Some(message))));
        let (rx, kind) = MessageKind::request();
        let envelope = Envelope::new(self.addr, message, kind);
        let result = object.send(self, envelope.upcast()).await;
        result
            .map_err(|err| SendError(Some(err.0.downcast().expect("impossible").into_message())))?;
        let envelope = rx.receive();
        let envelope = envelope.await.ok_or(SendError(None))?;
        Ok(envelope
            .downcast()
            .expect("invalid response")
            .into_message())
    }

    pub fn respond<R: Request>(
        &self,
        // TODO: rename `ReplyToken`?
        token: ReplyToken<R>,
        // TODO: support many responses.
        message: R::Response,
    ) -> Result<(), SendError<R>> {
        let tx = token.into_sender();
        let envelope = Envelope::new(self.addr, message, MessageKind::regular());
        tx.send(envelope.upcast())
            .map_err(|err| SendError(err.0.downcast().expect("impossible").into_message()))
    }

    #[inline]
    pub async fn recv(&self) -> Option<Envelope> {
        // TODO: cache `OwnedEntry`?
        let object = self.book.get_owned(self.addr)?;
        object.mailbox()?.recv().await
    }

    #[inline]
    pub fn try_recv(&self) -> Result<Envelope, TryRecvError> {
        let object = self.book.get(self.addr).ok_or(TryRecvError::Closed)?;
        object.mailbox().ok_or(TryRecvError::Closed)?.try_recv()
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

fn downcast<M: Message>(envelope: Envelope) -> M {
    envelope.downcast().expect("impossible").into_message()
}
