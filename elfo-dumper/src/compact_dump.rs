use serde::ser::{Serialize, SerializeStruct, Serializer};

use elfo_core::{
    dumping::{Dump, MessageKind},
    node,
};

pub(crate) struct CompactDump<'a> {
    dump: &'a Dump,
    class: &'a str,
    message_name: &'a str,
}

impl<'a> CompactDump<'a> {
    pub(crate) fn new(dump: &'a Dump, class: &'a str, buffer: &'a mut String) -> Self {
        Self {
            dump,
            class,
            message_name: dump.message_name.to_str(buffer),
        }
    }
}

impl<'a> Serialize for CompactDump<'a> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let field_count = 11
            + !self.dump.meta.key.is_empty() as usize // "k"
            + !matches!(self.dump.message_kind, MessageKind::Regular) as usize; // "c"

        let mut s = serializer.serialize_struct("Dump", field_count)?;

        // Dump `ts` firstly to make it possible to use `sort`.
        s.serialize_field("ts", &self.dump.timestamp)?;
        s.serialize_field("g", &self.dump.meta.group)?;

        if !self.dump.meta.key.is_empty() {
            s.serialize_field("k", &self.dump.meta.key)?;
        }

        s.serialize_field("n", &node::node_no())?;
        s.serialize_field("s", &self.dump.sequence_no)?;
        s.serialize_field("t", &self.dump.trace_id)?;
        s.serialize_field("th", &self.dump.thread_id)?;
        s.serialize_field("d", &self.dump.direction)?;
        s.serialize_field("cl", &self.class)?;
        s.serialize_field("mn", &self.message_name)?;
        s.serialize_field("mp", &self.dump.message_protocol)?;

        let (message_kind, correlation_id) = match self.dump.message_kind {
            MessageKind::Regular => ("Regular", None),
            MessageKind::Request(c) => ("Request", Some(c)),
            MessageKind::Response(c) => ("Response", Some(c)),
        };

        s.serialize_field("mk", message_kind)?;
        s.serialize_field("m", &*self.dump.message)?;

        if let Some(correlation_id) = correlation_id {
            s.serialize_field("c", &correlation_id)?;
        }

        s.end()
    }
}
