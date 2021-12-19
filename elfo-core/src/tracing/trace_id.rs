use std::{
    convert::TryFrom,
    num::{NonZeroU64, TryFromIntError},
};

use derive_more::{Display, From, Into};
use serde::{Deserialize, Serialize};

/// The struct that represents the trace id layout. It's built around
/// `NonZeroU64` for now.
// TODO(v0.2): remove `derive(Deserialize)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[derive(Serialize, Deserialize, Into, From, Display)]
#[display(fmt = "{}", _0)]
pub struct TraceId(NonZeroU64);

impl TryFrom<u64> for TraceId {
    type Error = TryFromIntError;

    #[inline]
    fn try_from(raw: u64) -> Result<Self, Self::Error> {
        NonZeroU64::try_from(raw).map(TraceId)
    }
}

impl From<TraceId> for u64 {
    #[inline]
    fn from(trace_id: TraceId) -> Self {
        trace_id.0.get()
    }
}
