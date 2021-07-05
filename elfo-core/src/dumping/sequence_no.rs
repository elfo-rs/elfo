use std::{
    num::NonZeroU64,
    sync::atomic::{AtomicU64, Ordering},
};

use crate::time::Timestamp;

pub struct SequenceNo(NonZeroU64);

pub(crate) struct SequenceNoGenerator {
    next_sequence_no: AtomicU64,
}

impl Default for SequenceNoGenerator {
    fn default() -> Self {
        let now = Timestamp::now().as_nanos();

        Self {
            // We add `1` here because `SequenceNo` is supposed to be non-zero.
            next_sequence_no: AtomicU64::new(now + 1),
        }
    }
}

impl SequenceNoGenerator {
    pub(crate) fn generate(&self) -> SequenceNo {
        let raw = self.next_sequence_no.fetch_add(1, Ordering::Relaxed);
        NonZeroU64::new(raw).map(SequenceNo).expect("impossible")
    }
}
