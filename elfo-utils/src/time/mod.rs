//! Provides [`Instant`] and [`SystemTime`] types.
//!
//! The main purpose is to provide a way to mock system/monotonic time in tests.

mod instant;
mod system;

pub use self::{instant::*, system::*};

#[cfg(any(test, feature = "test-util"))]
pub use mock::{with_mock, TimeMock};

#[cfg(any(test, feature = "test-util"))]
mod mock {
    use std::time::Duration;

    use super::*;

    /// Mocks `SystemTime` and `Instant`, see [`TimeMock`].
    pub fn with_mock(f: impl FnOnce(TimeMock)) {
        with_system_time_mock(|system| {
            with_instant_mock(|instant| {
                f(TimeMock { system, instant });
            });
        });
    }

    /// Controllable time source for use in tests.
    pub struct TimeMock {
        system: SystemTimeMock,
        instant: InstantMock,
    }

    impl TimeMock {
        /// Increase the time by the given duration.
        pub fn advance(&self, duration: Duration) {
            self.system.advance(duration);
            self.instant.advance(duration);
        }
    }
}
