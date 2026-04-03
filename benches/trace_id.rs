use std::{
    hint::black_box,
    time::{Duration, Instant},
};

use criterion::{Criterion, criterion_group, criterion_main};

use elfo::{Context, tracing::TraceId};

mod common;

fn generate(c: &mut Criterion) {
    async fn testee(_ctx: Context, iter_count: u64) -> Duration {
        let start = Instant::now();
        for _ in 0..iter_count {
            black_box(TraceId::generate());
        }
        start.elapsed()
    }

    c.bench_function("generate", |b| {
        b.iter_custom(|iter_count| common::bench_singleton(iter_count, testee))
    });
}

criterion_group!(cases, generate);
criterion_main!(cases);
