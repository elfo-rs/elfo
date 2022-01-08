use elfo_core::{
    dumping::{CheckResult, DumpItem, Recorder},
    scope,
};

use crate::storage::Registry;

impl Recorder for Registry {
    fn enabled(&self) -> bool {
        scope::try_with(|scope| match scope.dumping().check(self.class()) {
            CheckResult::Passed => {
                // TODO: `elfo_lost_dumps_total`
                // TODO: `elfo_emitted_dumps_total`
                true
            }
            CheckResult::NotInterested => false,
            CheckResult::Limited => {
                // TODO: `elfo_lost_dumps_total`
                false
            }
        })
        // TODO: limit dumps outside the actor system?
        .unwrap_or(false)
    }

    fn record(&self, dump: DumpItem) {
        self.add(dump);
    }
}
