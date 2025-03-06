use std::{
    convert::TryFrom,
    num::{NonZeroU64, TryFromIntError},
};

use derive_more::{Deref, Display, From, Into};
use serde::{Deserialize, Serialize};

use elfo_utils::time::SystemTime;

use crate::addr::NodeNo;

/// The struct that represents the trace id.
// TODO(v0.2): remove `derive(Deserialize)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[derive(Serialize, Deserialize, Into, From, Display)]
#[display("{_0}")]
pub struct TraceId(NonZeroU64);

impl TraceId {
    pub(crate) fn from_layout(layout: TraceIdLayout) -> Self {
        let raw = ((u64::from(*layout.timestamp)) << 38)
            | (u64::from(layout.node_no.map_or(0, |n| n.into_bits())) << 22)
            | u64::from(*layout.bottom);

        Self::try_from(raw).unwrap()
    }

    pub(crate) fn to_layout(self) -> TraceIdLayout {
        let raw = self.0.get();

        TraceIdLayout {
            timestamp: TruncatedTime((raw >> 38) as u32 & 0x1ff_ffff),
            node_no: NodeNo::from_bits(((raw >> 22) & 0xffff) as u16),
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
    pub(crate) node_no: Option<NodeNo>,
    pub(crate) bottom: Bottom,
}

/// 25-bit time in seconds.
#[derive(Clone, Copy, Deref)]
pub(crate) struct TruncatedTime(u32);

impl TruncatedTime {
    pub(crate) fn now() -> Self {
        let nanos = SystemTime::now().to_unix_time_secs();
        Self(nanos as u32 & 0x1ff_ffff)
    }

    pub(crate) fn abs_delta(self, other: TruncatedTime) -> u32 {
        let a = self.0.wrapping_sub(other.0) & 0x1ff_ffff;
        let b = other.0.wrapping_sub(self.0) & 0x1ff_ffff;
        a.min(b)
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
fn truncated_time_delta_secs() {
    let check = |a, b, expected| {
        assert_eq!(
            TruncatedTime(a).abs_delta(TruncatedTime(b)),
            expected,
            "{a} abs {b} != {expected}",
        );
    };

    check(5, 5, 0);
    check(25, 50, 25);
    check((1 << 25) - 15, (1 << 25) - 5, 10);
    check(0, (1 << 25) - 5, 5);
    check(1, (1 << 25) - 5, 6);
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
