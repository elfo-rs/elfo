use serde::ser::{Serialize, SerializeStruct, Serializer};

use elfo_core::{
    dumping::{Direction, Dump, ErasedMessage, MessageKind, SequenceNo, Timestamp},
    node,
    tracing::TraceId,
    ActorMeta,
};

pub(crate) struct CompactDump<'a> {
    meta: &'a ActorMeta,
    sequence_no: SequenceNo,
    timestamp: Timestamp,
    trace_id: TraceId,
    thread_id: u64,
    direction: Direction,
    class: &'a str,
    message_name: &'a str,
    message_protocol: &'a str,
    message_kind: MessageKind,
    message: &'a ErasedMessage,
}

impl<'a> CompactDump<'a> {
    pub(crate) fn new(dump: &'a Dump, class: &'a str, buffer: &'a mut String) -> Self {
        Self {
            meta: &*dump.meta,
            sequence_no: dump.sequence_no,
            timestamp: dump.timestamp,
            trace_id: dump.trace_id,
            thread_id: dump.thread_id,
            direction: dump.direction,
            class,
            message_name: dump.message_name.to_str(buffer),
            message_protocol: dump.message_protocol,
            message_kind: dump.message_kind,
            message: &dump.message,
        }
    }
}

impl<'a> Serialize for CompactDump<'a> {
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
        s.serialize_field("th", &self.thread_id)?;
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
        s.serialize_field("m", &**self.message)?;

        if let Some(correlation_id) = correlation_id {
            s.serialize_field("c", &correlation_id)?;
        }

        s.end()
    }
}
