use std::time::{Duration, Instant};

use criterion::{criterion_group, criterion_main, Criterion};

use elfo::Context;

mod common;

fn baseline(c: &mut Criterion) {
    async fn testee(iter_count: u64) -> Duration {
        let start = Instant::now();
        for _ in 0..iter_count {
            tokio::task::yield_now().await;
        }
        start.elapsed()
    }

    c.bench_function("baseline", |b| {
        b.iter_custom(|iter_count| {
            common::make_st_runtime().block_on(async {
                tokio::task::spawn(async move { testee(iter_count).await })
                    .await
                    .unwrap()
            })
        })
    });
}

async fn testee(_ctx: Context, iter_count: u64) -> Duration {
    let start = Instant::now();
    for _ in 0..iter_count {
        tokio::task::yield_now().await;
    }
    start.elapsed()
}

fn without_telemetry(c: &mut Criterion) {
    c.bench_function("without_telemetry", |b| {
        assert!(metrics::try_recorder().is_none());
        b.iter_custom(|iter_count| common::bench_singleton(iter_count, testee))
    });
}

// TODO: with stuck detection (requires runtime parameter).

fn with_telemetry(c: &mut Criterion) {
    c.bench_function("with_telemetry", |b| {
        // Install a recorder but don't run the telemeter actor.
        elfo::batteries::telemeter::init();
        assert!(metrics::try_recorder().is_some());
        b.iter_custom(|iter_count| common::bench_singleton(iter_count, testee))
    });
}

criterion_group!(cases, baseline, without_telemetry, with_telemetry);
criterion_main!(cases);
