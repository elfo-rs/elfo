use std::{
    sync::{Arc, Barrier},
    time::Instant,
};

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};

use elfo_utils::{RateLimit, RateLimiter};

fn rate_limiter(c: &mut Criterion) {
    let mut group = c.benchmark_group("rate_limiter");

    for n in 1..=8 {
        group.throughput(Throughput::Elements(1));
        group.bench_with_input(BenchmarkId::new("threads", n), &n, |b, &n| {
            b.iter_custom(|iter_count| {
                let barrier = Arc::new(Barrier::new(n));
                let limiter = Arc::new(RateLimiter::new(RateLimit::Rps(999_999_999)));

                let iter_per_thread_count = iter_count / n as u64;

                (0..n)
                    .map(|_| {
                        let barrier = barrier.clone();
                        let limiter = limiter.clone();

                        std::thread::spawn(move || {
                            barrier.wait();

                            let start_time = Instant::now();
                            for _ in 0..iter_per_thread_count {
                                assert!(limiter.acquire());
                            }
                            start_time.elapsed()
                        })
                    })
                    .collect::<Vec<_>>()
                    .into_iter()
                    .map(|h| h.join().unwrap())
                    .max()
                    .unwrap()
            })
        });
    }

    group.finish();
}

criterion_group!(benches, rate_limiter);
criterion_main!(benches);
