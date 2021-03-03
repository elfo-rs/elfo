use std::marker::PhantomData;

use crate::{
    addr::Addr,
    address_book::AddressBook,
    envelope::{Envelope, Message, MessageKind, ReplyToken, Request},
    mailbox::{SendError, TryRecvError},
};

pub struct Context<C, K> {
    addr: Addr,
    book: AddressBook,
    _config: PhantomData<C>,
    _key: PhantomData<K>,
}

impl<C, K> Clone for Context<C, K> {
    fn clone(&self) -> Self {
        Self {
            addr: self.addr,
            book: self.book.clone(),
            _config: PhantomData,
            _key: PhantomData,
        }
    }
}

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
        todo!()
    }

    pub async fn send_to<M: Message>(
        &self,
        recipient: Addr,
        message: M,
    ) -> Result<(), SendError<M>> {
        let object = ward!(self.book.get(recipient), return Err(SendError(message)));
        let envelope = Envelope::new(self.addr, message, MessageKind::regular());
        let result = object.send(envelope).await;
        result.map_err(|err| SendError(err.0.into_message()))
    }

    pub async fn ask<R: Request>(
        &self,
        recipient: Addr,
        message: R,
        // TODO: avoid `Option`.
    ) -> Result<Envelope<R::Response>, SendError<Option<R>>> {
        let book = &self.book;
        let object = ward!(book.get(recipient), return Err(SendError(Some(message))));
        let (rx, kind) = MessageKind::request();
        let envelope = Envelope::new(self.addr, message, kind);
        let result = object.send(envelope).await;
        result.map_err(|err| SendError(Some(err.0.into_message())))?;
        let envelope = rx.receive().await.ok_or(SendError(None))?;
        Ok(envelope.downcast().expect("invalid response"))
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
        let object = self.book.get(self.addr)?;
        object.mailbox()?.recv().await
    }

    #[inline]
    pub fn try_recv(&self) -> Result<Envelope, TryRecvError> {
        let object = self.book.get(self.addr).ok_or(TryRecvError::Closed)?;
        object.mailbox().ok_or(TryRecvError::Closed)?.try_recv()
    }
}

impl<C, K> Context<C, K> {
    pub(crate) fn root(book: AddressBook) -> Self {
        Self {
            addr: Addr::NULL,
            book,
            _config: PhantomData,
            _key: PhantomData,
        }
    }

    pub(crate) fn child(&self, addr: Addr) -> Self {
        Self {
            addr,
            book: self.book.clone(),
            _config: PhantomData,
            _key: PhantomData,
        }
    }
}
