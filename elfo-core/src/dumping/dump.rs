use std::sync::Arc;

use erased_serde::Serialize as ErasedSerialize;
use serde::{
    ser::{SerializeStruct, Serializer},
    Serialize,
};
use smallbox::{smallbox, SmallBox};

use elfo_macros::message;

use super::sequence_no::SequenceNoGenerator;
use crate::{
    actor::ActorMeta, dumping::sequence_no::SequenceNo, envelope, message::Message, node, scope,
    trace_id::TraceId,
};

#[doc(hidden)]
#[stability::unstable]
pub struct Dump {
    pub meta: Arc<ActorMeta>,
    pub sequence_no: SequenceNo,
    pub timestamp: Timestamp,
    pub trace_id: TraceId,
    pub direction: Direction,
    pub class: &'static str, // TODO: remove?
    pub message_name: &'static str,
    pub message_protocol: &'static str,
    pub message_kind: MessageKind,
    pub message: ErasedMessage,
}

assert_impl_all!(Dump: Send);
assert_eq_size!(Dump, [u8; 256]);

impl Dump {
    #[stability::unstable]
    pub fn builder() -> DumpBuilder {
        DumpBuilder {
            direction: Direction::Out,
            message_name: None,
            message_protocol: None,
            message_kind: MessageKind::Regular,
        }
    }

    pub(crate) fn message<M: Message>(
        message: M,
        kind: &envelope::MessageKind,
        direction: Direction,
    ) -> Self {
        Self::builder()
            .direction(direction)
            .message_name(M::NAME)
            .message_protocol(M::PROTOCOL)
            .message_kind(MessageKind::from_message_kind(kind))
            .finish(message)
    }
}

#[stability::unstable]
pub struct DumpBuilder {
    direction: Direction,
    message_name: Option<&'static str>,
    message_protocol: Option<&'static str>,
    message_kind: MessageKind,
}

impl DumpBuilder {
    #[stability::unstable]
    pub fn direction(&mut self, direction: Direction) -> &mut Self {
        self.direction = direction;
        self
    }

    #[stability::unstable]
    pub fn message_name(&mut self, name: &'static str) -> &mut Self {
        self.message_name = Some(name);
        self
    }

    #[stability::unstable]
    pub fn message_protocol(&mut self, protocol: &'static str) -> &mut Self {
        self.message_protocol = Some(protocol);
        self
    }

    #[stability::unstable]
    pub fn message_kind(&mut self, kind: MessageKind) -> &mut Self {
        self.message_kind = kind;
        self
    }

    #[stability::unstable]
    pub fn finish(&mut self, message: impl Serialize + Send + 'static) -> Dump {
        self.do_finish(smallbox!(message))
    }

    pub(crate) fn do_finish(&mut self, message: impl Into<ErasedMessage>) -> Dump {
        let (meta, trace_id) = scope::with(|scope| (scope.meta().clone(), scope.trace_id()));

        Dump {
            meta,
            sequence_no: SequenceNoGenerator::default().generate(), // TODO
            timestamp: Timestamp::now(),
            trace_id,
            direction: self.direction,
            class: "KEK",                                          // TODO
            message_name: self.message_name.unwrap_or(""),         // TODO
            message_protocol: self.message_protocol.unwrap_or(""), // TODO
            message_kind: self.message_kind,
            message: message.into(),
        }
    }
}

// TODO: move to `time`.
/// Timestamp in nanos since Unix epoch.
#[message(part, elfo = crate)]
#[derive(Copy, PartialEq, PartialOrd, Eq, Ord, Hash)]
#[stability::unstable]
pub struct Timestamp(u64);

impl Timestamp {
    #[cfg(not(test))]
    #[inline]
    #[stability::unstable]
    pub fn now() -> Self {
        let ns = std::time::UNIX_EPOCH
            .elapsed()
            .expect("invalid system time")
            .as_nanos() as u64;
        Self(ns)
    }

    #[cfg(test)]
    pub fn now() -> Self {
        Self(42)
    }

    #[inline]
    pub fn from_nanos(ns: u64) -> Self {
        Self(ns)
    }
}

#[doc(hidden)]
pub type ErasedMessage = SmallBox<dyn ErasedSerialize + Send, [u8; 136]>;

// Reexported in `elfo::_priv`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[stability::unstable]
pub enum Direction {
    In,
    Out,
}

// Reexported in `elfo::_priv`.
#[derive(Debug, Clone, Copy, PartialEq)]
#[stability::unstable]
pub enum MessageKind {
    Regular,
    Request(u64),
    Response(u64),
}

impl MessageKind {
    pub(crate) fn from_message_kind(kind: &crate::envelope::MessageKind) -> Self {
        use slotmap::Key;

        use crate::envelope::MessageKind as MK;

        match kind {
            MK::Regular { .. } => Self::Regular,
            MK::RequestAny(token) | MK::RequestAll(token) => {
                Self::Request(token.request_id.data().as_ffi())
            }
            MK::Response { request_id, .. } => Self::Response(request_id.data().as_ffi()),
        }
    }
}

// TODO: move to elfo-dumper
impl Serialize for Dump {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let field_count = 11
            + !self.meta.key.is_empty() as usize // "k"
            + !matches!(self.message_kind, MessageKind::Regular) as usize; // "c"

        let mut s = serializer.serialize_struct("Dump", field_count)?;

        // Dump `ts` firstly to make it possible to use `sort`.
        s.serialize_field("ts", &self.timestamp)?;
        s.serialize_field("g", &self.meta.group)?;

        if !self.meta.key.is_empty() {
            s.serialize_field("k", &self.meta.key)?;
        }

        s.serialize_field("n", &node::node_no())?;
        s.serialize_field("s", &self.sequence_no)?;
        s.serialize_field("t", &self.trace_id)?;
        s.serialize_field("d", &self.direction)?;
        s.serialize_field("cl", &self.class)?;
        s.serialize_field("mn", &self.message_name)?;
        s.serialize_field("mp", &self.message_protocol)?;

        let (message_kind, correlation_id) = match self.message_kind {
            MessageKind::Regular => ("Regular", None),
            MessageKind::Request(c) => ("Request", Some(c)),
            MessageKind::Response(c) => ("Response", Some(c)),
        };

        s.serialize_field("mk", message_kind)?;
        s.serialize_field("m", &*self.message)?;

        if let Some(correlation_id) = correlation_id {
            s.serialize_field("c", &correlation_id)?;
        }

        s.end()
    }
}
