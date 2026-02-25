use elfo_core::{
    dumping::{CheckResult, Dump, Recorder},
    scope,
};
use elfo_utils::ward;
use metrics::Key;

use crate::dump_storage::DumpRegistry;

impl Recorder for DumpRegistry {
    fn enabled(&self) -> bool {
        scope::try_with(|scope| {
            let dumping = scope.dumping();
            match dumping.check(self.class()) {
                CheckResult::Passed => true,
                CheckResult::NotInterested => false,
                CheckResult::Limited => {
                    // Consume a sequence number to create a detectable gap.
                    dumping.next_sequence_no();
                    emit_counter("elfo_limited_dumps_total");
                    false
                }
            }
        })
        // TODO: limit dumps outside the actor system?
        .unwrap_or(false)
    }

    fn record(&self, dump: Dump) {
        self.add(dump);
        emit_counter("elfo_emitted_dumps_total");
    }
}

fn emit_counter(name: &'static str) {
    let recorder = ward!(metrics::try_recorder());
    let key = Key::from_static_name(name);
    recorder.increment_counter(&key, 1);
}
