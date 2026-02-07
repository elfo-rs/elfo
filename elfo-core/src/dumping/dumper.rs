use std::sync::Arc;

use metrics::{Key, Label};

use crate::{scope, Message};

use super::{
    control::CheckResult,
    dump::*,
    recorder::{self, Recorder},
};

#[derive(Clone)]
#[instability::unstable]
pub struct Dumper {
    class: &'static str,
    recorder: Option<Arc<dyn Recorder>>,
}

impl Dumper {
    #[instability::unstable]
    pub fn new(class: &'static str) -> Self {
        Self {
            class,
            recorder: recorder::make_recorder(class),
        }
    }

    /// Records a dump if dumping is enabled for this class.
    ///
    /// The closure receives a [`DumpBuilder`] and must return a [`Dump`].
    /// It is not called when dumping is disabled or the message is
    /// rate-limited. Rate-limited messages still consume a sequence number,
    /// creating a detectable gap in the output (e.g. 1, 2, 5).
    #[inline]
    #[instability::unstable]
    pub fn record(&self, make_dump: impl FnOnce(DumpBuilder) -> Dump) {
        let r = ward!(self.recorder.as_deref());
        scope::try_with(|scope| {
            let dumping = scope.dumping();
            let seq_no = match dumping.check(self.class) {
                CheckResult::Passed => dumping.next_sequence_no(),
                CheckResult::Limited => {
                    // Consume, creating gap.
                    dumping.next_sequence_no();
                    // TODO: support class label?
                    counter("elfo_limited_dumps_total", &[]);
                    return;
                }
                CheckResult::NotInterested => return,
            };

            r.record(make_dump(Dump::builder(seq_no)));
            // TODO: support class label?
            counter("elfo_emitted_dumps_total", &[]);
        });
    }

    #[inline]
    pub(crate) fn record_m<M: Message>(
        &self,
        message: &M,
        make_dump: impl FnOnce(DumpBuilder) -> Dump,
    ) {
        if !message.dumping_allowed() {
            return;
        }
        self.record(make_dump);
    }
}

fn counter(name: &'static str, labels: &'static [Label]) {
    let recorder = ward!(metrics::try_recorder());
    let key = Key::from_static_labels(name, labels);
    recorder.increment_counter(&key, 1);
}
