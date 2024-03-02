use elfo_utils::time::Instant;

use crate::{
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
    RequestAny(ResponseToken),
    RequestAll(ResponseToken),
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
            MessageKind::RequestAny(token) => token.sender(),
            MessageKind::RequestAll(token) => token.sender(),
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

    #[inline]
    pub fn type_id(&self) -> std::any::TypeId {
        self.message.type_id()
    }

    #[doc(hidden)]
    #[stability::unstable]
    pub fn duplicate(&self) -> Self {
        Self {
            created_time: self.created_time,
            trace_id: self.trace_id,
            kind: match &self.kind {
                MessageKind::Regular { sender } => MessageKind::Regular { sender: *sender },
                MessageKind::RequestAny(token) => MessageKind::RequestAny(token.duplicate()),
                MessageKind::RequestAll(token) => MessageKind::RequestAll(token.duplicate()),
                MessageKind::Response { sender, request_id } => MessageKind::Response {
                    sender: *sender,
                    request_id: *request_id,
                },
            },
            message: self.message.clone(),
        }
    }

    // TODO: remove the method?
    pub(crate) fn set_message<M: Message>(&mut self, message: M) {
        self.message = message.upcast();
    }
}

// Extra traits to support both owned and borrowed usages of `msg!(..)`.

pub trait EnvelopeOwned {
    fn unpack_regular(self) -> AnyMessage;
    fn unpack_request(self) -> (AnyMessage, ResponseToken);
}

pub trait EnvelopeBorrowed {
    fn unpack_regular(&self) -> &AnyMessage;
}

impl EnvelopeOwned for Envelope {
    #[inline]
    fn unpack_regular(self) -> AnyMessage {
        #[cfg(feature = "network")]
        if let MessageKind::RequestAny(token) | MessageKind::RequestAll(token) = self.kind {
            // The sender thought this is a request, but for the current node it isn't.
            // Mark the token as received to return `RequestError::Ignored` to the sender.
            let _ = token.into_received::<()>();
        }

        #[cfg(not(feature = "network"))]
        debug_assert!(!matches!(
            self.kind,
            MessageKind::RequestAny(_) | MessageKind::RequestAll(_)
        ));

        self.message
    }

    #[inline]
    fn unpack_request(self) -> (AnyMessage, ResponseToken) {
        match self.kind {
            MessageKind::RequestAny(token) | MessageKind::RequestAll(token) => {
                (self.message, token)
            }
            // A request sent by using `ctx.send()` ("fire and forget").
            // Also it's useful for the protocol evolution between remote nodes.
            _ => (self.message, ResponseToken::forgotten()),
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
    fn downcast2<M: Message>(self) -> M {
        match self.downcast::<M>() {
            Ok(message) => message,
            Err(message) => panic!("unexpected message: {message:?}"),
        }
    }
}

impl AnyMessageBorrowed for AnyMessage {
    #[inline]
    fn downcast2<M: Message>(&self) -> &M {
        ward!(
            self.downcast_ref::<M>(),
            panic!("unexpected message: {self:?}")
        )
    }
}
