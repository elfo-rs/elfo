//! Provides the [`Instant`] type.
//!
//! The main purpose of this module is to abstract over quanta/minstant/etc.
//!
//! Another purpose is to provide a way to mock instant time in tests.

use std::time::Duration;

use quanta::Clock;

/// A measurement of a monotonically nondecreasing clock.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Instant(u64 /* raw TSC value */); // TODO: make it `NonZeroU64`?

impl Instant {
    /// Returns the current monotonic time.
    #[inline]
    pub fn now() -> Self {
        Self(with_clock(|c| c.raw()))
    }

    /// Returns the amount of time elapsed since this instant.
    ///
    /// Prefer `elapsed_secs_f64()` if used for metrics.
    pub fn elapsed(&self) -> Duration {
        Self::now().duration_since(*self)
    }

    /// Returns the number of seconds elapsed since this instant.
    ///
    /// This method saturates to zero.
    pub fn elapsed_secs_f64(&self) -> f64 {
        Self::now().secs_f64_since(*self)
    }

    /// Returns the number of nanoseconds elapsed since this instant.
    ///
    /// This method saturates to zero.
    pub fn elapsed_nanos(&self) -> u64 {
        Self::now().nanos_since(*self)
    }

    /// Returns the amount of time elapsed from another instant to this one.
    ///
    /// This method saturates to zero.
    ///
    /// Prefer `secs_f64_since()` if used for metrics.
    #[inline]
    pub fn duration_since(&self, earlier: Self) -> Duration {
        with_clock(|c| c.delta(earlier.0, self.0))
    }

    /// Returns the number of seconds elapsed from another instant to this one.
    ///
    /// This method saturates to zero.
    #[inline]
    pub fn secs_f64_since(&self, earlier: Self) -> f64 {
        self.nanos_since(earlier) as f64 * 1e-9
    }

    /// Returns the number of nanosecs elapsed from another instant to this one.
    ///
    /// This method saturates to zero.
    #[inline]
    pub fn nanos_since(&self, earlier: Self) -> u64 {
        with_clock(|c| c.delta_as_nanos(earlier.0, self.0))
    }
}

pub(crate) fn nanos_since_unknown_epoch() -> u64 {
    with_clock(|c| c.delta_as_nanos(0, c.raw()))
}

fn with_clock<R>(f: impl FnOnce(&Clock) -> R) -> R {
    use std::sync::OnceLock;

    static CLOCK: OnceLock<Clock> = OnceLock::new();

    #[cfg(any(test, feature = "test-util"))]
    return mock::CLOCK.with(|c| match c.borrow().as_ref() {
        Some(c) => f(c),
        None => f(CLOCK.get_or_init(Clock::new)),
    });

    #[cfg(not(any(test, feature = "test-util")))]
    f(CLOCK.get_or_init(Clock::new))
}

#[cfg(any(test, feature = "test-util"))]
pub use mock::{with_instant_mock, InstantMock};

#[cfg(any(test, feature = "test-util"))]
mod mock {
    use std::cell::RefCell;

    use super::*;

    thread_local! {
        pub(super) static CLOCK: RefCell<Option<Clock>> = const { RefCell::new(None) };
    }

    /// Mocks `Instant`, see [`InstantMock`].
    pub fn with_instant_mock<R>(f: impl FnOnce(InstantMock) -> R) -> R {
        let (clock, mock) = Clock::mock();
        let mock = InstantMock(mock);
        CLOCK.with(|c| *c.borrow_mut() = Some(clock));
        let result = f(mock);
        CLOCK.with(|c| *c.borrow_mut() = None);
        result
    }

    /// Controllable time source for use in tests.
    pub struct InstantMock(std::sync::Arc<quanta::Mock>);

    impl InstantMock {
        /// Increase the time by the given duration.
        pub fn advance(&self, duration: Duration) {
            self.0.increment(duration);
        }
    }
}
