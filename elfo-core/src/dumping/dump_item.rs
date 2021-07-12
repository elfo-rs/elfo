use std::sync::Arc;

use erased_serde::Serialize as ErasedSerialize;
use serde::{
    ser::{SerializeStruct, Serializer},
    Serialize,
};
use smallbox::SmallBox;

use crate::{
    dumping::sequence_no::SequenceNo, object::ObjectMeta, time::Timestamp, trace_id::TraceId,
};

// Reexported in `elfo::_priv`.
pub struct DumpItem {
    pub meta: Arc<ObjectMeta>,
    pub sequence_no: SequenceNo,
    pub timestamp: Timestamp,
    pub trace_id: TraceId,
    pub direction: Direction,
    pub class: &'static str,
    pub message_name: &'static str,
    pub message_protocol: &'static str,
    pub message_kind: MessageKind,
    pub message: ErasedMessage,
}

pub type ErasedMessage = SmallBox<dyn ErasedSerialize + Send, [u8; 136]>;

assert_impl_all!(DumpItem: Send);
assert_eq_size!(DumpItem, [u8; 256]);

// Reexported in `elfo::_priv`.
#[derive(Debug, PartialEq, Serialize)]
pub enum Direction {
    In,
    Out,
}

// Reexported in `elfo::_priv`.
#[derive(Debug, PartialEq)]
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
        }
    }
}

impl Serialize for DumpItem {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let field_count = 11
            + self.meta.key.is_some() as usize
            + !matches!(self.message_kind, MessageKind::Regular) as usize;

        let mut s = serializer.serialize_struct("Dump", field_count)?;
        s.serialize_field("g", &self.meta.group)?;

        if let Some(key) = &self.meta.key {
            s.serialize_field("k", key)?;
        }

        s.serialize_field("n", &0)?; // TODO: node_no
        s.serialize_field("s", &self.sequence_no)?;
        s.serialize_field("t", &self.trace_id)?;
        s.serialize_field("ts", &self.timestamp)?;
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
