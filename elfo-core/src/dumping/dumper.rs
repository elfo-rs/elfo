use std::{sync::Arc, time::Duration};

use smallbox::smallbox;
use tracing::error;

use super::{
    dump_item::*,
    recorder::{self, Recorder},
    sequence_no::SequenceNoGenerator,
};
use crate::{
    envelope,
    message::{Message, Request},
    request_table::RequestId,
    scope,
};

#[derive(Clone)]
#[stability::unstable]
pub struct Dumper {
    class: &'static str,
    recorder: Option<Arc<dyn Recorder>>,
}

impl Dumper {
    pub fn new(class: &'static str) -> Self {
        Self {
            class,
            recorder: recorder::make_recorder(class),
        }
    }

    // TODO: naming
    #[inline]
    #[stability::unstable]
    pub fn is_enabled(&self) -> bool {
        self.recorder.as_ref().map_or(false, |r| r.enabled())
    }

    #[inline(always)]
    pub(crate) fn dump_message<M: Message>(
        &self,
        message: &M,
        kind: &envelope::MessageKind,
        direction: Direction,
    ) {
        self.dump(
            direction,
            M::NAME,
            M::PROTOCOL,
            MessageKind::from_message_kind(kind),
            smallbox!(message.clone()),
        );
    }

    #[inline(always)]
    pub(crate) fn dump_response<R: Request>(
        &self,
        message: &R::Response,
        request_id: RequestId,
        direction: Direction,
    ) {
        use slotmap::Key;

        self.dump(
            direction,
            R::Wrapper::NAME,
            R::Wrapper::PROTOCOL,
            MessageKind::Response(request_id.data().as_ffi()),
            smallbox!(message.clone()),
        );
    }

    #[stability::unstable]
    pub fn dump(
        &self,
        direction: Direction,
        message_name: &'static str,
        message_protocol: &'static str,
        message_kind: MessageKind,
        message: ErasedMessage,
    ) {
        let d = scope::try_with(|scope| (scope.meta().clone(), scope.trace_id()));

        let (meta, trace_id) = ward!(d, {
            cooldown!(Duration::from_secs(15), {
                error!("attempt to dump outside the actor scope");
            });
            return;
        });

        let item = DumpItem {
            meta,
            sequence_no: SequenceNoGenerator::default().generate(), // TODO
            timestamp: Timestamp::now(),
            trace_id,
            direction,
            class: self.class,
            message_name,
            message_protocol,
            message_kind,
            message,
        };

        let recorder = self.recorder.as_ref().expect("dump() without is_enabled()");
        recorder.record(item);
    }
}
