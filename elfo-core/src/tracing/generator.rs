use std::{
    cell::Cell,
    convert::TryFrom,
    sync::atomic::{AtomicU64, Ordering},
    time::SystemTime,
};

use super::TraceId;
use crate::{node, time};

static NEXT_CHUNK_NO: AtomicU64 = AtomicU64::new(0);

thread_local! {
    static PREV_TRACE_ID: Cell<TraceId> = Cell::new(TraceId::try_from(0x3ff).unwrap());
}

/// Generates a new trace id according to the next layout:
/// * 1  bit  0 (zero)
/// * 25 bits timestamp in secs
/// * 16 bits node_no
/// * 12 bits (chunk_no & 0xfff)
/// * 10 bits counter
pub(crate) fn generate_trace_id() -> TraceId {
    PREV_TRACE_ID.with(|cell| {
        let new_trace_id = do_generate(cell.get());
        cell.set(new_trace_id);
        new_trace_id
    })
}

fn do_generate(prev: TraceId) -> TraceId {
    let raw = u64::from(prev);
    let ts = time::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("invalid system time")
        .as_secs();

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
    let node_no = u64::from(node::node_no());
    let chunk_no = NEXT_CHUNK_NO.fetch_add(1, Ordering::Relaxed);
    node_no << 22 | (chunk_no & 0xfff) << 10 | 1
}

#[test]
fn it_works() {
    use std::time::Duration;

    let sec = 1 << 38;
    let st = u64::from(generate_trace_id());

    time::advance(Duration::from_millis(500));
    assert_eq!(u64::from(generate_trace_id()), st + 1);
    time::advance(Duration::from_millis(500));
    assert_eq!(u64::from(generate_trace_id()), st + sec + 2);
    time::advance(Duration::from_millis(500));
    assert_eq!(u64::from(generate_trace_id()), st + sec + 3);
    time::advance(Duration::from_millis(500));
    assert_eq!(u64::from(generate_trace_id()), st + 2 * sec + 4);

    std::thread::spawn(move || {
        time::advance(Duration::from_secs(2));
        is_divisible_by_chunk(u64::from(generate_trace_id()) - (st + 2 * sec));
    })
    .join()
    .unwrap();

    for i in 5..1023 {
        assert_eq!(u64::from(generate_trace_id()), st + 2 * sec + i);
    }

    is_divisible_by_chunk(u64::from(generate_trace_id()) - (st + 2 * sec));

    fn is_divisible_by_chunk(diff: u64) {
        assert_eq!((diff / (1 << 10)) * (1 << 10), diff);
    }
}
