use std::{
    convert::TryFrom,
    num::{NonZeroU64, TryFromIntError},
    time::SystemTime,
};

use derive_more::{Deref, Display, From, Into};
use serde::{Deserialize, Serialize};

use crate::node::NodeNo;

/// The struct that represents the trace id layout. It's built around
/// `NonZeroU64` for now.
// TODO(v0.2): remove `derive(Deserialize)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[derive(Serialize, Deserialize, Into, From, Display)]
#[display(fmt = "{}", _0)]
pub struct TraceId(NonZeroU64);

impl TraceId {
    pub(crate) fn from_layout(layout: TraceIdLayout) -> Self {
        let raw = (u64::from(*layout.timestamp)) << 38
            | u64::from(layout.node_no) << 22
            | u64::from(*layout.bottom);

        Self::try_from(raw).unwrap()
    }

    pub(crate) fn to_layout(self) -> TraceIdLayout {
        let raw = self.0.get();

        TraceIdLayout {
            timestamp: TruncatedTime((raw >> 38) as u32 & 0x1ff_ffff),
            node_no: (raw >> 22 & 0xffff) as NodeNo,
            bottom: Bottom((raw & 0x3f_ffff) as u32),
        }
    }
}

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

// === TraceIdLayout ===

#[derive(Clone, Copy)]
pub(crate) struct TraceIdLayout {
    pub(crate) timestamp: TruncatedTime,
    pub(crate) node_no: NodeNo,
    pub(crate) bottom: Bottom,
}

/// 25-bit time in seconds.
#[derive(Clone, Copy, Deref)]
pub(crate) struct TruncatedTime(u32);

impl From<SystemTime> for TruncatedTime {
    fn from(time: SystemTime) -> Self {
        let unixtime = time
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("invalid system time")
            .as_secs();

        Self(unixtime as u32 & 0x1ff_ffff)
    }
}

/// 22-bit bottom part (usually, a counter).
#[derive(Clone, Copy, Deref)]
pub(crate) struct Bottom(u32);

impl From<u32> for Bottom {
    fn from(value: u32) -> Self {
        debug_assert!((1..=0x3f_ffff).contains(&value));
        Self(value)
    }
}

#[test]
fn layout_roundtrip() {
    fn check(raw_trace_id: u64) {
        let trace_id = TraceId::try_from(raw_trace_id).unwrap();
        let layout = trace_id.to_layout();
        assert_eq!(TraceId::from_layout(layout), trace_id);
    }

    check(1);
    check(9223372036854775807);

    // Just random numbers.
    check(36203675);
    check(75997165362483795);
    check(276561162443934569);
    check(2765611624439345695);
    check(5197794958151101819);
    check(8446744073709551614);
}
