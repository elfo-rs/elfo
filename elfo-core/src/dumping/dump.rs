use std::{borrow::Cow, fmt, sync::Arc, time::SystemTime};

use erased_serde::Serialize as ErasedSerialize;
use serde::Serialize;
use smallbox::{smallbox, SmallBox};

use elfo_macros::message;

use super::{extract_name::extract_name, sequence_no::SequenceNo};
use crate::{actor::ActorMeta, envelope, scope, trace_id::TraceId};

// === Dump ===

#[doc(hidden)]
#[stability::unstable]
pub struct Dump {
    pub meta: Arc<ActorMeta>,
    pub sequence_no: SequenceNo,
    pub timestamp: Timestamp,
    pub trace_id: TraceId,
    pub direction: Direction,
    pub message_name: MessageName,
    pub message_protocol: &'static str,
    pub message_kind: MessageKind,
    pub message: ErasedMessage,
}

#[doc(hidden)]
#[stability::unstable]
pub type ErasedMessage = SmallBox<dyn ErasedSerialize + Send, [u8; 200]>;

assert_impl_all!(Dump: Send);
assert_eq_size!(Dump, [u8; 320]);

impl Dump {
    #[stability::unstable]
    pub fn builder() -> DumpBuilder {
        DumpBuilder {
            timestamp: None,
            direction: Direction::Out,
            message_name: None,
            message_protocol: "",
            message_kind: MessageKind::Regular,
        }
    }

    pub(crate) fn message<M: crate::Message>(
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

// === DumpBuilder ===

#[stability::unstable]
pub struct DumpBuilder {
    timestamp: Option<Timestamp>,
    direction: Direction,
    message_name: Option<MessageName>,
    message_protocol: &'static str,
    message_kind: MessageKind,
}

impl DumpBuilder {
    #[stability::unstable]
    pub fn timestamp(&mut self, timestamp: impl Into<Timestamp>) -> &mut Self {
        self.timestamp = Some(timestamp.into());
        self
    }

    #[stability::unstable]
    pub fn direction(&mut self, direction: Direction) -> &mut Self {
        self.direction = direction;
        self
    }

    #[stability::unstable]
    pub fn message_name(&mut self, name: impl Into<MessageName>) -> &mut Self {
        self.message_name = Some(name.into());
        self
    }

    #[stability::unstable]
    pub fn message_protocol(&mut self, protocol: &'static str) -> &mut Self {
        self.message_protocol = protocol;
        self
    }

    #[stability::unstable]
    pub fn message_kind(&mut self, kind: MessageKind) -> &mut Self {
        self.message_kind = kind;
        self
    }

    #[stability::unstable]
    pub fn finish<M>(&mut self, message: M) -> Dump
    where
        M: Serialize + Send + 'static,
    {
        if self.message_name.is_none() {
            // If the simplest serializer fails, the actual serialization will fail too.
            self.message_name = Some(extract_name(&message));
        }

        self.do_finish(smallbox!(message))
    }

    pub(crate) fn do_finish(&mut self, message: ErasedMessage) -> Dump {
        let (meta, trace_id, sequence_no) = scope::with(|scope| {
            (
                scope.meta().clone(),
                scope.trace_id(),
                scope.dumping().next_sequence_no(),
            )
        });

        Dump {
            meta,
            sequence_no,
            timestamp: self.timestamp.unwrap_or_else(Timestamp::now),
            trace_id,
            direction: self.direction,
            message_name: self.message_name.take().unwrap_or_default(),
            message_protocol: self.message_protocol,
            message_kind: self.message_kind,
            message,
        }
    }
}

// === Timestamp ===

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
        SystemTime::now().into()
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

impl From<SystemTime> for Timestamp {
    fn from(sys_time: SystemTime) -> Self {
        let ns = sys_time
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64;

        Self(ns)
    }
}

// === Direction ===

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[stability::unstable]
pub enum Direction {
    In,
    Out,
}

// === MessageName ===

#[derive(Debug, Clone, Default, PartialEq)]
#[stability::unstable]
pub struct MessageName(&'static str, Option<&'static str>);

impl fmt::Display for MessageName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)?;

        if let Some(variant) = self.1 {
            f.write_str("::")?;
            f.write_str(variant)?;
        }

        Ok(())
    }
}

impl From<&'static str> for MessageName {
    #[inline]
    fn from(struct_name: &'static str) -> Self {
        Self(struct_name, None)
    }
}

impl From<(&'static str, &'static str)> for MessageName {
    #[inline]
    fn from((enum_name, variant): (&'static str, &'static str)) -> Self {
        Self(enum_name, Some(variant))
    }
}

/// Useful for metrics.
impl From<MessageName> for Cow<'static, str> {
    #[inline]
    fn from(name: MessageName) -> Self {
        Self::from(&name)
    }
}

impl From<&MessageName> for Cow<'static, str> {
    #[inline]
    fn from(name: &MessageName) -> Self {
        match name.1 {
            Some(variant) => {
                let mut string = String::with_capacity(name.0.len() + 2 + variant.len());
                string.push_str(name.0);
                string.push_str("::");
                string.push_str(variant);
                Self::Owned(string)
            }
            None => Self::Borrowed(name.0),
        }
    }
}

impl MessageName {
    #[doc(hidden)]
    #[stability::unstable]
    pub fn to_str<'a>(&self, buffer: &'a mut String) -> &'a str {
        if let Some(variant) = self.1 {
            buffer.clear();
            buffer.push_str(self.0);
            buffer.push_str("::");
            buffer.push_str(variant);
            &buffer[..]
        } else {
            self.0
        }
    }
}

// === MessageKind ===

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
