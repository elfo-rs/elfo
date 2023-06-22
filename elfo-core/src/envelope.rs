use quanta::Instant;

use crate::{
    address_book::AddressBook,
    message::{AnyMessage, Message},
    request_table::{RequestId, ResponseToken},
    tracing::TraceId,
    Addr,
};

// TODO: use granular messages instead of `SmallBox`.
#[derive(Debug)]
pub struct Envelope<M = AnyMessage> {
    created_time: Instant, // Now used also as a sent time.
    trace_id: TraceId,
    kind: MessageKind,
    message: M,
}

assert_impl_all!(Envelope: Send);
assert_eq_size!(Envelope, [u8; 256]);

// Reexported in `elfo::_priv`.
#[derive(Debug)]
pub enum MessageKind {
    Regular { sender: Addr },
    RequestAny(ResponseToken<()>),
    RequestAll(ResponseToken<()>),
    Response { sender: Addr, request_id: RequestId },
}

impl<M> Envelope<M> {
    // This is private API. Do not use it.
    #[doc(hidden)]
    #[inline]
    pub fn new(message: M, kind: MessageKind) -> Self {
        Self::with_trace_id(message, kind, crate::scope::trace_id())
    }

    // This is private API. Do not use it.
    #[doc(hidden)]
    #[inline]
    pub fn with_trace_id(message: M, kind: MessageKind, trace_id: TraceId) -> Self {
        Self {
            created_time: Instant::now(),
            trace_id,
            kind,
            message,
        }
    }

    #[inline]
    pub fn trace_id(&self) -> TraceId {
        self.trace_id
    }

    #[inline]
    pub fn message(&self) -> &M {
        &self.message
    }

    // This is private API. Do not use it.
    #[doc(hidden)]
    #[inline]
    pub fn into_message(self) -> M {
        self.message
    }

    /// Part of private API. Do not use it.
    #[doc(hidden)]
    pub fn message_kind(&self) -> &MessageKind {
        &self.kind
    }

    pub(crate) fn created_time(&self) -> Instant {
        self.created_time
    }

    #[inline]
    pub fn sender(&self) -> Addr {
        match &self.kind {
            MessageKind::Regular { sender } => *sender,
            MessageKind::RequestAny(token) => token.sender,
            MessageKind::RequestAll(token) => token.sender,
            MessageKind::Response { sender, .. } => *sender,
        }
    }
}

impl<M: Message> Envelope<M> {
    // This is private API. Do not use it.
    #[doc(hidden)]
    pub fn upcast(self) -> Envelope {
        Envelope {
            created_time: self.created_time,
            trace_id: self.trace_id,
            kind: self.kind,
            message: self.message.upcast(),
        }
    }
}

impl Envelope {
    #[inline]
    pub fn is<M: Message>(&self) -> bool {
        self.message.is::<M>()
    }

    pub(crate) fn do_downcast<M: Message>(self) -> Envelope<M> {
        let message = self.message.downcast::<M>().expect("cannot downcast");
        Envelope {
            created_time: self.created_time,
            trace_id: self.trace_id,
            kind: self.kind,
            message,
        }
    }

    // XXX: why does `Envelope` know about `AddressBook`?
    // TODO: avoid `None` here?
    #[doc(hidden)]
    #[stability::unstable]
    pub fn duplicate(&self, book: &AddressBook) -> Option<Self> {
        Some(Self {
            created_time: self.created_time,
            trace_id: self.trace_id,
            kind: match &self.kind {
                MessageKind::Regular { sender } => MessageKind::Regular { sender: *sender },
                MessageKind::RequestAny(token) => {
                    let object = book.get(token.sender)?;
                    let token = object.as_actor()?.request_table().clone_token(token)?;
                    MessageKind::RequestAny(token)
                }
                MessageKind::RequestAll(token) => {
                    let object = book.get(token.sender)?;
                    let token = object.as_actor()?.request_table().clone_token(token)?;
                    MessageKind::RequestAll(token)
                }
                MessageKind::Response { sender, request_id } => MessageKind::Response {
                    sender: *sender,
                    request_id: *request_id,
                },
            },
            message: self.message.clone(),
        })
    }

    // TODO: remove the method?
    pub(crate) fn set_message<M: Message>(&mut self, message: M) {
        self.message = message.upcast();
    }
}

// Extra traits to support both owned and borrowed usages of `msg!(..)`.

pub trait EnvelopeOwned {
    fn unpack_regular(self) -> AnyMessage;
    fn unpack_request<T>(self) -> (AnyMessage, ResponseToken<T>);
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
    fn unpack_request<T>(self) -> (AnyMessage, ResponseToken<T>) {
        match self.kind {
            // A request sent by using `ctx.send()` ("fire and forget").
            // Also it's useful for the protocol evolution.
            MessageKind::Regular { .. } => (self.message, ResponseToken::forgotten()),
            MessageKind::RequestAny(token) => (self.message, token.into_typed()),
            MessageKind::RequestAll(token) => (self.message, token.into_typed()),
            MessageKind::Response { .. } => unreachable!(),
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
    fn downcast2<M: Message>(self) -> M;
}

pub trait AnyMessageBorrowed {
    fn downcast2<M: Message>(&self) -> &M;
}

impl AnyMessageOwned for AnyMessage {
    #[inline]
    #[track_caller]
    fn downcast2<M: Message>(self) -> M {
        match self.downcast::<M>() {
            Ok(message) => message,
            Err(message) => panic!("unexpected message: {message:?}"),
        }
    }
}

impl AnyMessageBorrowed for AnyMessage {
    #[inline]
    #[track_caller]
    fn downcast2<M: Message>(&self) -> &M {
        ward!(
            self.downcast_ref::<M>(),
            panic!("unexpected message: {self:?}")
        )
    }
}
