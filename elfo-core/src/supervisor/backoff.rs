use std::time::Duration;

use elfo_utils::time::Instant;

const BACKOFF_STEP: Duration = Duration::from_secs(5);
const MAX_BACKOFF: Duration = Duration::from_secs(30);

pub(crate) struct Backoff {
    next_backoff: Duration,
    start_time: Instant,
}

impl Default for Backoff {
    fn default() -> Self {
        Self {
            next_backoff: BACKOFF_STEP,
            start_time: Instant::now(),
        }
    }
}

impl Backoff {
    pub(crate) fn start(&mut self) {
        self.start_time = Instant::now();
    }

    pub(crate) fn next(&mut self) -> Duration {
        // If an actor is alive enough time, reset the backoff.
        if self.start_time.elapsed() >= BACKOFF_STEP {
            self.next_backoff = Duration::ZERO;
        }

        let backoff = self.next_backoff;
        self.next_backoff = (self.next_backoff + BACKOFF_STEP).min(MAX_BACKOFF);
        backoff
    }
}

#[cfg(test)]
mod tests {
    use elfo_utils::time;

    use super::*;

    #[test]
    fn it_works() {
        time::with_instant_mock(|mock| {
            let mut backoff = Backoff::default();

            // Immediately failed.
            assert_eq!(backoff.next(), BACKOFF_STEP);
            mock.advance(BACKOFF_STEP);
            backoff.start();

            // And again.
            assert_eq!(backoff.next(), 2 * BACKOFF_STEP);
            mock.advance(2 * BACKOFF_STEP);
            backoff.start();

            // After some, not enough to reset the backoff, time.
            mock.advance(BACKOFF_STEP * 2 / 3);
            assert_eq!(backoff.next(), 3 * BACKOFF_STEP);
            mock.advance(3 * BACKOFF_STEP);
            backoff.start();

            // After some, enough to reset the backoff, time.
            mock.advance(BACKOFF_STEP);
            assert_eq!(backoff.next(), Duration::ZERO); // resetted
            backoff.start();

            // After some, not enough to reset the backoff, time.
            mock.advance(BACKOFF_STEP * 2 / 3);
            assert_eq!(backoff.next(), BACKOFF_STEP);
        });
    }
}
