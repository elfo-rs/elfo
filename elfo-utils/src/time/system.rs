//! Provides the [`SystemTime`] type.
//!
//! The main purpose is to provide a way to mock system time in tests.

use std::time::SystemTime as StdSystemTime;

/// A measurement of a system clock.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct SystemTime(u64 /* unix time nanos */); // TODO: make it `NonZeroU64`?

impl SystemTime {
    /// Represents "1970-01-01 00:00:00 UTC".
    pub const UNIX_EPOCH: Self = Self(0);

    /// Returns the current system time.
    #[inline]
    pub fn now() -> Self {
        #[cfg(any(test, feature = "test-util"))]
        if let Some(now_ns) = mock::NOW_NS.with(|t| t.get()) {
            return Self(now_ns);
        }

        StdSystemTime::now().into()
    }

    /// Returns the number of seconds since the unix epoch.
    #[inline]
    pub fn to_unix_time_secs(&self) -> u64 {
        self.to_unix_time_nanos() / 1_000_000_000
    }

    /// Returns the number of nanoseconds since the unix epoch.
    #[inline]
    pub fn to_unix_time_nanos(&self) -> u64 {
        self.0
    }
}

impl From<StdSystemTime> for SystemTime {
    fn from(sys_time: StdSystemTime) -> Self {
        let unix_time_ns = sys_time
            .duration_since(StdSystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64;

        Self(unix_time_ns)
    }
}

#[cfg(any(test, feature = "test-util"))]
pub use mock::{with_system_time_mock, SystemTimeMock};

#[cfg(any(test, feature = "test-util"))]
mod mock {
    use std::{cell::Cell, time::Duration};

    thread_local! {
        pub(super) static NOW_NS: Cell<Option<u64>> = const { Cell::new(None) };
    }

    /// Mocks `SystemTime`, see [`SystemTimeMock`].
    pub fn with_system_time_mock(f: impl FnOnce(SystemTimeMock)) {
        NOW_NS.with(|t| t.set(Some(0)));
        f(SystemTimeMock);
        NOW_NS.with(|t| t.set(None));
    }

    /// Controllable time source for use in tests.
    #[non_exhaustive]
    pub struct SystemTimeMock;

    impl SystemTimeMock {
        /// Increase the time by the given duration.
        pub fn advance(&self, duration: Duration) {
            NOW_NS.with(|t| {
                let mut now_ns = t.get().expect("use of moved system time mock");
                now_ns += duration.as_nanos() as u64;
                t.set(Some(now_ns));
            })
        }
    }
}
