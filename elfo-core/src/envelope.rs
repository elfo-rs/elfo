use std::marker::PhantomData;

use futures_intrusive::channel::shared::{self, OneshotReceiver, OneshotSender};
use smallbox::smallbox;

use crate::{
    addr::Addr,
    message::{with_vtable, AnyMessage, LocalTypeId, Message},
    trace_id::TraceId,
};

// TODO: use granular messages instead of `SmallBox`.
#[derive(Debug)]
pub struct Envelope<M = AnyMessage> {
    trace_id: TraceId,
    sender: Addr,
    kind: MessageKind,
    ltid: LocalTypeId,
    message: M,
}

assert_eq_size!(Envelope, [u8; 128]);

#[derive(Debug)]
pub(crate) enum MessageKind {
    Regular,
    Request(OneshotSender<Envelope>),
}

impl MessageKind {
    #[inline]
    pub(crate) fn regular() -> Self {
        MessageKind::Regular
    }

    #[inline]
    pub(crate) fn request() -> (OneshotReceiver<Envelope>, Self) {
        let (tx, rx) = shared::oneshot_channel();
        (rx, MessageKind::Request(tx))
    }
}

#[must_use]
pub struct ReplyToken<T> {
    tx: OneshotSender<Envelope>,
    marker: PhantomData<T>,
}

impl<T> ReplyToken<T> {
    #[doc(hidden)]
    #[inline]
    pub fn from_sender(tx: OneshotSender<Envelope>) -> Self {
        let marker = PhantomData;
        Self { tx, marker }
    }

    pub(crate) fn into_sender(self) -> OneshotSender<Envelope> {
        self.tx
    }
}

impl<M: Message> Envelope<M> {
    pub(crate) fn new(sender: Addr, message: M, kind: MessageKind) -> Self {
        Self {
            trace_id: TraceId::new(1).unwrap(), // TODO: load trace_id.
            sender,
            kind,
            ltid: M::_LTID,
            message,
        }
    }

    #[inline]
    pub fn sender(&self) -> Addr {
        self.sender
    }

    #[inline]
    pub fn upcast(self) -> Envelope {
        Envelope {
            trace_id: self.trace_id,
            sender: self.sender,
            kind: self.kind,
            ltid: self.ltid,
            message: smallbox!(self.message),
        }
    }

    #[inline]
    pub fn into_message(self) -> M {
        self.message
    }
}

impl Envelope {
    #[inline]
    pub fn is<M: Message>(&self) -> bool {
        self.message.is::<M>()
    }

    #[inline]
    pub fn downcast<M: Message>(self) -> Result<Envelope<M>, Envelope> {
        match self.message.downcast::<M>() {
            Ok(message) => Ok(Envelope {
                trace_id: self.trace_id,
                sender: self.sender,
                kind: self.kind,
                ltid: self.ltid,
                message: message.into_inner(),
            }),
            Err(message) => Err(Envelope {
                trace_id: self.trace_id,
                sender: self.sender,
                kind: self.kind,
                ltid: self.ltid,
                message,
            }),
        }
    }

    pub(crate) fn clone(&self) -> Self {
        Self {
            trace_id: self.trace_id,
            sender: self.sender,
            kind: MessageKind::Regular, // TODO
            ltid: self.ltid,
            message: with_vtable(self.ltid, |vtable| (vtable.clone)(&self.message)),
        }
    }
}

// Extra traits to support both owned and borrowed usages of `msg!(..)`.

pub trait EnvelopeOwned {
    fn unpack_regular(self) -> AnyMessage;
    fn unpack_request(self) -> (AnyMessage, OneshotSender<Envelope>);
}

pub trait EnvelopeBorrowed {
    fn unpack_regular(&self) -> &AnyMessage;
}

impl EnvelopeOwned for Envelope {
    #[inline]
    fn unpack_regular(self) -> AnyMessage {
        self.message
    }

    #[inline]
    fn unpack_request(self) -> (AnyMessage, OneshotSender<Envelope>) {
        match self.kind {
            MessageKind::Request(tx) => (self.message, tx),
            _ => unreachable!(),
        }
    }
}

impl EnvelopeBorrowed for Envelope {
    #[inline]
    fn unpack_regular(&self) -> &AnyMessage {
        &self.message
    }
}

pub trait AnyMessageOwned {
    fn downcast2<T: 'static>(self) -> T;
}

pub trait AnyMessageBorrowed {
    fn downcast2<T: 'static>(&self) -> &T;
}

impl AnyMessageOwned for AnyMessage {
    #[inline]
    fn downcast2<T: 'static>(self) -> T {
        self.downcast::<T>().unwrap().into_inner()
    }
}

impl AnyMessageBorrowed for AnyMessage {
    #[inline]
    fn downcast2<T: 'static>(&self) -> &T {
        self.downcast_ref::<T>().unwrap()
    }
}
