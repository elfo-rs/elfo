use std::{
    convert::TryFrom,
    num::{NonZeroU64, TryFromIntError},
    sync::atomic::{AtomicU64, Ordering},
};

use serde::Serialize;

// TODO: make it just type alias (or not?)
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct SequenceNo(NonZeroU64);

impl TryFrom<u64> for SequenceNo {
    type Error = TryFromIntError;

    #[inline]
    fn try_from(raw: u64) -> Result<Self, Self::Error> {
        NonZeroU64::try_from(raw).map(Self)
    }
}

impl From<SequenceNo> for u64 {
    #[inline]
    fn from(sequence_no: SequenceNo) -> Self {
        sequence_no.0.get()
    }
}

pub(crate) struct SequenceNoGenerator {
    next_sequence_no: AtomicU64,
}

impl Default for SequenceNoGenerator {
    fn default() -> Self {
        Self {
            // We starts with `1` here because `SequenceNo` is supposed to be non-zero.
            next_sequence_no: AtomicU64::new(1),
        }
    }
}

impl SequenceNoGenerator {
    pub(crate) fn generate(&self) -> SequenceNo {
        let raw = self.next_sequence_no.fetch_add(1, Ordering::Relaxed);
        NonZeroU64::new(raw).map(SequenceNo).expect("impossible")
    }
}
