use std::sync::Arc;

use erased_serde::Serialize as ErasedSerialize;
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
    pub message: SmallBox<dyn ErasedSerialize + Send, [u8; 136]>,
}

assert_impl_all!(DumpItem: Send);
assert_eq_size!(DumpItem, [u8; 256]);

// Reexported in `elfo::_priv`.
pub enum Direction {
    In,
    Out,
}

// Reexported in `elfo::_priv`.
pub enum MessageKind {
    Regular,
    Request(u64),
    Response(u64),
}
