use std::sync::Arc;

use crate::Message;

use super::{
    dump::*,
    recorder::{self, Recorder},
};

#[derive(Clone)]
#[instability::unstable]
pub struct Dumper {
    recorder: Option<Arc<dyn Recorder>>,
}

impl Dumper {
    #[instability::unstable]
    pub fn new(class: &'static str) -> Self {
        Self {
            recorder: recorder::make_recorder(class),
        }
    }

    #[inline]
    #[instability::unstable]
    pub fn acquire(&self) -> Option<DumpingPermit<'_>> {
        let r = self.recorder.as_deref().filter(|r| r.enabled())?;
        Some(DumpingPermit { recorder: r })
    }

    pub(crate) fn acquire_m<M: Message>(&self, message: &M) -> Option<DumpingPermit<'_>> {
        if !message.dumping_allowed() {
            return None;
        }

        self.acquire()
    }
}

#[must_use]
#[instability::unstable]
pub struct DumpingPermit<'a> {
    recorder: &'a dyn Recorder,
}

impl DumpingPermit<'_> {
    #[instability::unstable]
    pub fn record(self, dump: Dump) {
        self.recorder.record(dump);
    }
}
