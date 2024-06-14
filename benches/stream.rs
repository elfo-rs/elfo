use std::time::{Duration, Instant};

use criterion::{black_box, criterion_group, criterion_main, Criterion};

use elfo::{message, stream::Stream, Context};

mod common;

#[message]
struct SomeEvent(usize);

fn futures03(c: &mut Criterion) {
    async fn testee(mut ctx: Context, iter_count: u64) -> Duration {
        ctx.attach(Stream::from_futures03(futures::stream::iter(
            (0..).map(SomeEvent),
        )));

        let start = Instant::now();
        for _ in 0..iter_count {
            black_box(ctx.recv().await.unwrap());
        }
        start.elapsed()
    }

    c.bench_function("futures03", |b| {
        b.iter_custom(|iter_count| common::bench_singleton(iter_count, testee))
    });
}

fn generate(c: &mut Criterion) {
    async fn testee(mut ctx: Context, iter_count: u64) -> Duration {
        ctx.attach(Stream::generate(move |mut e| async move {
            for i in 0.. {
                e.emit(SomeEvent(i)).await;
            }
        }));

        let start = Instant::now();
        for _ in 0..iter_count {
            black_box(ctx.recv().await.unwrap());
        }
        start.elapsed()
    }

    c.bench_function("generate", |b| {
        b.iter_custom(|iter_count| common::bench_singleton(iter_count, testee))
    });
}

criterion_group!(cases, futures03, generate);
criterion_main!(cases);
