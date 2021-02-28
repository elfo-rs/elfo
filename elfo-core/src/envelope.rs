use std::any::Any;

use futures_intrusive::channel::shared::{self, OneshotReceiver, OneshotSender};
use smallbox::{smallbox, SmallBox};

use crate::{addr::Addr, trace_id::TraceId};

// TODO: granular messages.

#[derive(Debug, Clone)]
pub struct Envelope<M = AnyMessage> {
    trace_id: TraceId,
    sender: Addr,
    kind: MessageKind,
    message: M,
}

pub trait Message: Any + Send + Clone {}

// TODO: make private?
#[derive(Debug)]
pub enum MessageKind {
    Regular,
    Request(OneshotSender<Envelope>),
}

impl Clone for MessageKind {
    #[inline]
    fn clone(&self) -> Self {
        Self::Regular // TODO
    }
}

impl MessageKind {
    #[inline]
    pub fn regular() -> Self {
        MessageKind::Regular
    }

    #[inline]
    pub fn request() -> (OneshotReceiver<Envelope>, Self) {
        let (tx, rx) = shared::oneshot_channel();
        (rx, MessageKind::Request(tx))
    }
}

pub(crate) type AnyMessage = SmallBox<dyn Any + Send, [u8; 88]>;

impl<M> Envelope<M> {
    #[inline]
    pub fn new(sender: Addr, message: M, kind: MessageKind) -> Self {
        Self {
            trace_id: TraceId::new(1).unwrap(), // TODO: load trace_id.
            sender,
            kind,
            message,
        }
    }

    #[inline]
    pub fn sender(&self) -> Addr {
        self.sender
    }

    // XXX
    #[cfg(feature = "test-util")]
    pub(crate) fn downgrade_to_reqular(&mut self) -> Option<OneshotSender<Envelope>> {
        match std::mem::replace(&mut self.kind, MessageKind::Regular) {
            MessageKind::Request(tx) => Some(tx),
            _ => None,
        }
    }
}

impl<M: Message> Envelope<M> {
    #[inline]
    pub fn upcast(self) -> Envelope {
        Envelope {
            trace_id: self.trace_id,
            sender: self.sender,
            kind: self.kind,
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
    pub fn downcast<M: Message>(self) -> Result<Envelope<M>, Envelope> {
        match self.message.downcast::<M>() {
            Ok(message) => Ok(Envelope {
                trace_id: self.trace_id,
                sender: self.sender,
                kind: self.kind,
                message: message.into_inner(),
            }),
            Err(message) => Err(Envelope {
                trace_id: self.trace_id,
                sender: self.sender,
                kind: self.kind,
                message,
            }),
        }
    }
}

#[test]
fn envelope_size() {
    assert_eq!(std::mem::size_of::<Envelope>(), 128);
}
