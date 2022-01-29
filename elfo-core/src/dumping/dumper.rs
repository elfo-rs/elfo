use std::sync::Arc;

use super::{
    dump::*,
    recorder::{self, Recorder},
};

#[derive(Clone)]
#[stability::unstable]
pub struct Dumper {
    recorder: Option<Arc<dyn Recorder>>,
}

impl Dumper {
    pub fn new(class: &'static str) -> Self {
        Self {
            recorder: recorder::make_recorder(class),
        }
    }

    #[inline]
    #[stability::unstable]
    pub fn acquire(&self) -> Option<DumpingPermit<'_>> {
        let r = self.recorder.as_deref().filter(|r| r.enabled())?;
        Some(DumpingPermit { recorder: &*r })
    }
}

#[must_use]
#[stability::unstable]
pub struct DumpingPermit<'a> {
    recorder: &'a dyn Recorder,
}

impl DumpingPermit<'_> {
    #[stability::unstable]
    pub fn record(self, dump: Dump) {
        self.recorder.record(dump);
    }
}
