use std::time::Duration;

use crate::RestartParams;
use elfo_utils::time::Instant;

pub(crate) struct RestartBackoff {
    start_time: Instant,
    restart_count: u64,
    power: u64,
}

impl Default for RestartBackoff {
    fn default() -> Self {
        Self {
            start_time: Instant::now(),
            restart_count: 0,
            power: 0,
        }
    }
}

impl RestartBackoff {
    pub(crate) fn start(&mut self) {
        self.start_time = Instant::now();
    }

    pub(crate) fn next(&mut self, params: &RestartParams) -> Option<Duration> {
        // If an actor is alive enough time, reset the backoff.
        if self.start_time.elapsed() >= params.auto_reset {
            self.restart_count = 1;
            self.power = 0;
            return Some(Duration::ZERO);
        }
        self.restart_count += 1;

        if self.restart_count > params.max_retries.get() {
            return None;
        }

        let delay = (params.min_backoff.as_secs_f64() * params.factor.powf(self.power as f64))
            .clamp(
                params.min_backoff.as_secs_f64(),
                params.max_backoff.as_secs_f64(),
            );

        self.power += 1;
        // Check for overflow, if overflow is detected set the current delay to maximal
        let delay = if delay.is_finite() {
            Duration::from_secs_f64(delay)
        } else {
            params.max_backoff
        };

        Some(delay)
    }
}

#[cfg(test)]
mod tests {
    use elfo_utils::time;
    use std::num::NonZeroU64;

    use super::*;

    #[test]
    fn it_works() {
        time::with_instant_mock(|mock| {
            let mut backoff = RestartBackoff::default();
            let params = RestartParams::new(Duration::from_secs(5), Duration::from_secs(30))
                .max_retries(NonZeroU64::new(3).unwrap());
            // Immediately failed.
            assert_eq!(backoff.next(&params), Some(params.min_backoff));
            mock.advance(params.min_backoff);
            backoff.start();

            // And again.
            assert_eq!(backoff.next(&params), Some(2 * params.min_backoff));
            mock.advance(2 * params.min_backoff);
            backoff.start();

            // After some, not enough to reset the backoff, time.
            mock.advance(params.min_backoff * 2 / 3);
            assert_eq!(backoff.next(&params), Some(4 * params.min_backoff));
            mock.advance(3 * params.min_backoff);
            backoff.start();

            // After some, enough to reset the backoff, time.
            mock.advance(params.min_backoff);
            // The first retry.
            assert_eq!(backoff.next(&params), Some(Duration::ZERO)); // resetted
            backoff.start();

            // After some, not enough to reset the backoff, time.
            mock.advance(params.min_backoff * 2 / 3);
            // The second retry.
            assert_eq!(backoff.next(&params), Some(params.min_backoff));
            // The third retry.
            assert_eq!(backoff.next(&params), Some(2 * params.min_backoff));
            // We reached the limit of reties.
            assert_eq!(backoff.next(&params), None);
        });
    }

    #[test]
    fn correctness() {
        let mut backoff = RestartBackoff::default();
        // Start with zero backoff duration.
        let params = RestartParams::new(Duration::from_secs(0), Duration::from_secs(0));
        assert_eq!(backoff.next(&params), Some(Duration::ZERO));
        assert_eq!(backoff.next(&params), Some(Duration::ZERO));
        assert_eq!(backoff.next(&params), Some(Duration::ZERO));

        // Then check the transition from zero to nonzero backoff limits.
        let params = RestartParams::new(Duration::from_secs(2), Duration::from_secs(16));
        assert_eq!(backoff.next(&params), Some(params.min_backoff));
        assert_eq!(backoff.next(&params), Some(2 * params.min_backoff));
        assert_eq!(backoff.next(&params), Some(4 * params.min_backoff));

        // Decreasing the upper bound results in a reduced subsequent backoff.
        let params = RestartParams::new(Duration::from_secs(3), Duration::from_secs(5));
        assert_eq!(backoff.next(&params), Some(params.max_backoff));

        // Increasing the lower bound raises the subsequent backoff.
        let params = RestartParams::new(Duration::from_secs(20), Duration::from_secs(30));
        assert_eq!(backoff.next(&params), Some(params.max_backoff));

        // Limiting the number of retry attempts kicks in.
        let mut backoff = RestartBackoff::default();
        let params = RestartParams::new(Duration::from_secs(20), Duration::from_secs(30))
            .max_retries(NonZeroU64::new(2).unwrap());
        assert_eq!(backoff.next(&params), Some(params.min_backoff));
        assert_eq!(backoff.next(&params), Some(params.max_backoff));
        assert_eq!(backoff.next(&params), None);
    }
}
