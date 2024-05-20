use std::{
    sync::atomic::{AtomicU64, Ordering::Relaxed},
    time::Duration,
};

use crate::time;

/// A rate limiter implementing [GCRA](https://en.wikipedia.org/wiki/Generic_cell_rate_algorithm).
pub struct RateLimiter {
    step: AtomicU64,
    period: AtomicU64,
    vtime: AtomicU64,
}

/// Unlimited by default.
impl Default for RateLimiter {
    fn default() -> Self {
        Self::new(RateLimit::Unlimited)
    }
}

/// A rate limit configuration.
#[derive(Clone, Copy)]
pub enum RateLimit {
    /// Unlimited rate.
    Unlimited,
    /// Requests per second.
    Rps(u64),
    /// Requests per a custom period.
    Custom(u64, Duration),
}

impl RateLimit {
    fn step_and_period(self) -> (u64, u64) {
        let (limit, period) = match self {
            Self::Unlimited => (1, 1),
            Self::Rps(rps) => (rps, SEC),
            Self::Custom(limit, period) => (limit, period.as_nanos() as u64),
        };

        (calculate_step(limit, period), period)
    }
}

const SEC: u64 = 1_000_000_000;
const UNLIMITED: u64 = 0;
const DISABLED: u64 = u64::MAX;

impl RateLimiter {
    /// Creates a new limiter.
    pub fn new(limit: RateLimit) -> Self {
        let (step, period) = limit.step_and_period();

        Self {
            step: AtomicU64::new(step),
            period: AtomicU64::new(period),
            vtime: AtomicU64::new(0),
        }
    }

    /// Reconfigures a limiter.
    pub fn configure(&self, limit: RateLimit) {
        let (step, period) = limit.step_and_period();

        self.step.store(step, Relaxed);
        self.period.store(period, Relaxed);
    }

    /// Resets a limiter.
    pub fn reset(&self) {
        self.vtime.store(0, Relaxed);
    }

    /// Acquires one permit.
    /// Returns `true` if an operation is allowed.
    #[inline]
    pub fn acquire(&self) -> bool {
        let step = self.step.load(Relaxed);

        // Handle special cases.
        if step == UNLIMITED {
            return true;
        }
        if step == DISABLED {
            return false;
        }

        let period = self.period.load(Relaxed);
        let now = time::nanos_since_unknown_epoch();
        let deadline = now + period;

        // GCRA logic.
        self.vtime
            // It seems to be enough to use `Relaxed` here.
            .fetch_update(Relaxed, Relaxed, |vtime| {
                if vtime < deadline {
                    Some(vtime.max(now) + step)
                } else {
                    None
                }
            })
            .is_ok()
    }
}

fn calculate_step(max_rate: u64, period: u64) -> u64 {
    if max_rate == 0 {
        return DISABLED;
    }

    // Practically unlimited.
    if max_rate >= period {
        return UNLIMITED;
    }

    // round_up(period / max_rate)
    (period - 1) / max_rate + 1
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ns(ns: u64) -> Duration {
        Duration::from_nanos(ns)
    }

    #[test]
    fn step_calculation() {
        for period in [1, 100, 1000] {
            assert_eq!(calculate_step(0, period), DISABLED);
            assert_eq!(calculate_step(period, period), UNLIMITED);

            for coef in 2..50 {
                assert_eq!(calculate_step(period, coef * period), coef);
                assert_eq!(calculate_step(period, coef * period + 1), coef + 1);
            }
        }
    }

    #[test]
    fn forbidding() {
        time::with_instant_mock(|mock| {
            let limiter = RateLimiter::new(RateLimit::Rps(0));
            for _ in 0..=5 {
                assert!(!limiter.acquire());
                mock.advance(ns(SEC));
            }
        });
    }

    #[test]
    fn unlimited() {
        time::with_instant_mock(|_mock| {
            let limiter = RateLimiter::new(RateLimit::Unlimited);
            let limiter2 = RateLimiter::new(RateLimit::Rps(1_000_000_000));
            let limiter3 = RateLimiter::new(RateLimit::Custom(2_000, Duration::from_micros(2)));
            for _ in 0..=1_000_000 {
                assert!(limiter.acquire());
                assert!(limiter2.acquire());
                assert!(limiter3.acquire());
            }
        });
    }

    #[test]
    fn limited() {
        for limit in [1, 2, 3, 4, 5, 17, 100, 1_000, 1_013] {
            time::with_instant_mock(|mock| {
                let limiter = RateLimiter::new(RateLimit::Rps(limit));

                for _ in 0..=5 {
                    for _ in 0..limit {
                        assert!(limiter.acquire());
                    }
                    assert!(!limiter.acquire());
                    mock.advance(ns(SEC));
                }
            });
        }
    }

    #[test]
    fn keeps_rate() {
        for limit in [1, 5, 25, 50] {
            time::with_instant_mock(|mock| {
                let limiter = RateLimiter::new(RateLimit::Rps(limit));

                // Skip the first second.
                for _ in 0..limit {
                    assert!(limiter.acquire());
                }
                assert!(!limiter.acquire());

                let parts = 10;
                let mut counter = 0;

                for _ in 0..(10 * parts) {
                    mock.advance(ns(SEC / parts));
                    while limiter.acquire() {
                        counter += 1;
                    }
                }

                assert_eq!(counter, 10 * limit, "{limit}");
            });
        }
    }

    #[test]
    fn reset() {
        time::with_instant_mock(|mock| {
            let limit = 10;
            let limiter = RateLimiter::new(RateLimit::Rps(limit));

            for _ in 0..=5 {
                for _ in 0..limit {
                    assert!(limiter.acquire());
                }
                limiter.reset();
                for _ in 0..limit {
                    assert!(limiter.acquire());
                }
                assert!(!limiter.acquire());
                mock.advance(ns(SEC));
            }
        });
    }
}
