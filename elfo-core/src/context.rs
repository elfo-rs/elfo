use std::marker::PhantomData;

use crate::{
    addr::Addr,
    address_book::AddressBook,
    envelope::{Envelope, MessageKind, ReplyToken},
    mailbox::{SendError, TryRecvError},
    message::{Message, Request},
};

pub struct Context<C = (), K = ()> {
    addr: Addr,
    book: AddressBook,
    key: K,
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

    pub async fn ask<R: Request>(
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

    pub fn reply<R: Request>(
        &self,
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

    pub(crate) fn child<C1, K1>(&self, addr: Addr, key: K1) -> Context<C1, K1> {
        Context {
            addr,
            book: self.book.clone(),
            key,
            _config: PhantomData,
        }
    }
}

impl Context<(), ()> {
    pub fn root() -> Self {
        Self {
            addr: Addr::NULL,
            book: AddressBook::new(),
            key: (),
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
        }
    }
}
