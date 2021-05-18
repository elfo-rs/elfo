use std::{
    cell::Cell,
    convert::TryFrom,
    num::{NonZeroU64, TryFromIntError},
    sync::atomic::{AtomicU16, AtomicU64, Ordering},
    time::Duration,
};

use derive_more::{Display, From, Into};
use serde::{Deserialize, Serialize};

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

static NEXT_CHUNK_NO: AtomicU64 = AtomicU64::new(0);
static NODE_NO: AtomicU16 = AtomicU16::new(65535);

thread_local! {
    static PREV_TRACE_ID: Cell<TraceId> = Cell::new(TraceId::try_from(0x3ff).unwrap());
}

/// Sets the generator to generate trace ids with the provided `node_no`.
/// The value `65535` is used if isn't called.
pub fn set_node_no(node_no: u16) {
    NODE_NO.store(node_no, Ordering::Relaxed)
}

/// Generates a new trace id according to the next layout:
/// * 1  bit  0 (zero)
/// * 25 bits timestamp in secs
/// * 16 bits node_no
/// * 12 bits (chunk_no & 0xfff)
/// * 10 bits counter
pub fn generate() -> TraceId {
    PREV_TRACE_ID.with(|cell| {
        let new_trace_id = do_generate(cell.get());
        cell.set(new_trace_id);
        new_trace_id
    })
}

fn do_generate(prev: TraceId) -> TraceId {
    let raw = prev.0.get();
    let ts = time::now().as_secs();

    // Check whether the chunk is exhausted.
    let not_ts = if raw & 0x3ff == 0x3ff {
        next_chunk()
    } else {
        (raw & 0x3f_ffff_ffff) + 1
    };

    TraceId::try_from((ts & 0x1ff_ffff) << 38 | not_ts).expect("impossible")
}

#[cold]
fn next_chunk() -> u64 {
    let node_no = u64::from(NODE_NO.load(Ordering::Relaxed));
    let chunk_no = NEXT_CHUNK_NO.fetch_add(1, Ordering::Relaxed);
    node_no << 22 | (chunk_no & 0xfff) << 10 | 1
}

#[cfg(test)]
mod time {
    use super::*;

    static NOW_MS: AtomicU64 = AtomicU64::new(0);

    pub(crate) fn advance(ms: u64) {
        NOW_MS.fetch_add(ms, Ordering::SeqCst);
    }

    pub(crate) fn now() -> Duration {
        let ms = NOW_MS.load(Ordering::SeqCst);
        Duration::from_millis(ms)
    }
}

#[cfg(not(test))]
mod time {
    use super::*;

    #[inline]
    pub(crate) fn now() -> Duration {
        std::time::UNIX_EPOCH
            .elapsed()
            .expect("invalid system time")
    }
}

#[test]
fn it_works() {
    let sec = 1 << 38;
    let chunk = 1 << 10;
    let st = u64::from(generate());

    time::advance(500);
    assert_eq!(u64::from(generate()), st + 1);
    time::advance(500);
    assert_eq!(u64::from(generate()), st + sec + 2);
    time::advance(500);
    assert_eq!(u64::from(generate()), st + sec + 3);
    time::advance(500);
    assert_eq!(u64::from(generate()), st + 2 * sec + 4);

    std::thread::spawn(move || {
        assert_eq!(u64::from(generate()), st + 2 * sec + chunk);
    })
    .join()
    .unwrap();

    for i in 5..1023 {
        assert_eq!(u64::from(generate()), st + 2 * sec + i);
    }

    assert_eq!(u64::from(generate()), st + 2 * sec + 2 * chunk);
}
