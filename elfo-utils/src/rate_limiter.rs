use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

use quanta::Instant;

pub struct RateLimiter {
    start_time: Instant,
    step: AtomicU64,
    vtime: AtomicU64, // TODO: wrap into `CachePadded`?
}

const SEC: u64 = 1_000_000_000;

impl RateLimiter {
    pub fn new(max_rate: u64) -> Self {
        Self {
            start_time: Instant::now(),
            step: AtomicU64::new(calculate_step(SEC, max_rate)),
            vtime: AtomicU64::new(0),
        }
    }

    pub fn configure(&self, max_rate: u64) {
        self.step.store(calculate_step(SEC, max_rate), Relaxed);
    }

    #[inline]
    pub fn acquire(&self) -> bool {
        let step = self.step.load(Relaxed);
        if step == 0 {
            return false;
        }

        let now = (Instant::now() - self.start_time).as_nanos() as u64;

        // It seems to be enough to use `Relaxed` here.
        self.vtime
            .fetch_update(Relaxed, Relaxed, |vtime| {
                if vtime < now + SEC {
                    Some(vtime.max(now) + step)
                } else {
                    None
                }
            })
            .is_ok()
    }
}

fn calculate_step(period: u64, max_rate: u64) -> u64 {
    // The special case, handled in `acquire()`.
    if max_rate == 0 {
        return 0;
    }

    // Practically unlimited.
    if max_rate >= period {
        return 1;
    }

    // round_up(period / max_rate)
    (period - 1) / max_rate + 1
}

#[cfg(test)]
mod tests {
    use quanta::{Clock, Mock};

    use super::*;

    fn with_time_mock(f: impl FnOnce(&Mock)) {
        let (clock, mock) = Clock::mock();
        quanta::with_clock(&clock, || f(&mock));
    }

    #[test]
    fn forbidden() {
        with_time_mock(|mock| {
            let limiter = RateLimiter::new(0);
            for _ in 0..=5 {
                assert!(!limiter.acquire());
                mock.increment(SEC);
            }
        });
    }

    #[test]
    fn unlimited() {
        with_time_mock(|_mock| {
            let limiter = RateLimiter::new(SEC);
            for _ in 0..=1_000_000 {
                assert!(limiter.acquire());
            }
        });
    }

    #[test]
    fn limited() {
        for limit in [1, 2, 3, 4, 5, 17, 100, 1_000, 1_013] {
            with_time_mock(|mock| {
                let limiter = RateLimiter::new(limit);

                for _ in 0..=5 {
                    for _ in 0..limit {
                        assert!(limiter.acquire());
                    }
                    assert!(!limiter.acquire());
                    mock.increment(SEC);
                }
            });
        }
    }

    #[test]
    fn keeps_rate() {
        for limit in [1, 5, 25, 50] {
            with_time_mock(|mock| {
                let limiter = RateLimiter::new(limit);

                // Skip the first second.
                for _ in 0..limit {
                    assert!(limiter.acquire());
                }
                assert!(!limiter.acquire());

                let parts = 10;
                let mut counter = 0;

                for _ in 0..(10 * parts) {
                    mock.increment(SEC / parts);
                    while limiter.acquire() {
                        counter += 1;
                    }
                }

                assert_eq!(counter, 10 * limit, "{}", limit);
            });
        }
    }
}
