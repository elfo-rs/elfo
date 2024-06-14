use std::{
    future::{poll_fn, Future},
    pin::pin,
    time::Duration,
};

use criterion::{criterion_group, criterion_main, Criterion};

use elfo::Context;
use elfo_utils::time::Instant;

mod common;

async fn testee(_ctx: Context, iter_count: u64) -> Duration {
    // Avoid measuring the time spent in `yield_now()` by accumulating the time
    // elapsed between `Poll::Pending` in order to measure only elfo's overhead.
    let mut elapsed = Duration::from_secs(0);
    let mut origin = None;

    for _ in 0..iter_count {
        if origin.is_none() {
            origin = Some(Instant::now());
        }

        poll_fn(|cx| {
            let mut fut = pin!(elfo::coop::consume_budget());
            let res = fut.as_mut().poll(cx);

            if res.is_pending() {
                elapsed += origin.unwrap().elapsed();
                origin = None;
            }

            res
        })
        .await;
    }

    // If `iter_count` is too small to trigger `yield_now()`.
    if let Some(origin) = origin {
        elapsed += origin.elapsed();
    }

    elapsed
}

fn by_count(c: &mut Criterion) {
    c.bench_function("by_count", |b| {
        assert!(metrics::try_recorder().is_none());
        b.iter_custom(|iter_count| common::bench_singleton(iter_count, testee))
    });
}

fn by_time(c: &mut Criterion) {
    c.bench_function("by_time", |b| {
        let _ = metrics::set_recorder(&metrics::NoopRecorder);
        b.iter_custom(|iter_count| common::bench_singleton(iter_count, testee))
    });
}

criterion_group!(cases, by_count, by_time);
criterion_main!(cases);
