//! Provides the [`SystemTime`] type.
//!
//! The main purpose is to provide a way to mock system time in tests.

#[cfg(feature = "system-time-source")]
use std::sync::OnceLock;
use std::time::{Duration, SystemTime as StdSystemTime};

#[cfg(feature = "system-time-source")]
static NOW: OnceLock<fn() -> SystemTime> = OnceLock::new();

/// Installs a process-wide source for [`SystemTime::now`].
///
/// Returns `true` if installed, or `false` if a source was already installed.
/// The source cannot be replaced or removed and is called on the calling
/// thread. It must not call [`SystemTime::now`], which would recurse.
#[cfg(feature = "system-time-source")]
#[cfg_attr(docsrs, doc(cfg(feature = "system-time-source")))]
pub fn set_system_time_source(now: fn() -> SystemTime) -> bool {
    NOW.set(now).is_ok()
}

/// A measurement of a system clock.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct SystemTime(u64 /* unix time nanos */); // TODO: make it `NonZeroU64`?

impl SystemTime {
    /// Represents "1970-01-01 00:00:00 UTC".
    pub const UNIX_EPOCH: Self = Self(0);

    /// Returns the current system time.
    ///
    /// With the `system-time-source` feature, uses the installed time source, or
    /// the system clock by default. A thread-local test mock takes precedence
    /// over either source.
    #[inline]
    pub fn now() -> Self {
        #[cfg(any(test, feature = "test-util"))]
        if let Some(now_ns) = mock::NOW_NS.with(|t| t.get()) {
            return Self(now_ns);
        }

        #[cfg(feature = "system-time-source")]
        if let Some(now) = NOW.get() {
            return now();
        }

        StdSystemTime::now().into()
    }

    /// Creates an instance based on nanoseconds since the unix epoch.
    #[inline]
    pub fn from_unix_time_nanos(nanos: u64) -> Self {
        Self(nanos)
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

impl From<SystemTime> for StdSystemTime {
    fn from(sys_time: SystemTime) -> Self {
        StdSystemTime::UNIX_EPOCH + Duration::from_nanos(sys_time.0)
    }
}

#[cfg(any(test, feature = "test-util"))]
pub use mock::{SystemTimeMock, with_system_time_mock};

#[cfg(any(test, feature = "test-util"))]
mod mock {
    use std::cell::Cell;

    use super::*;

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
