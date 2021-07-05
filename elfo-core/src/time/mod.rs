use std::{
    ops::{Add, Sub},
    u64,
};

#[cfg(feature = "test-util")]
pub use tokio::time::{advance, pause, resume};
pub use tokio::time::{Duration, Instant};

use elfo_macros::message;

pub use self::{interval::Interval, stopwatch::Stopwatch};

mod interval;
mod stopwatch;

// TODO: use `quanta`.

/// Timestamp in nanos since Unix epoch.
#[message(part, elfo = crate)]
#[derive(Copy, PartialEq, PartialOrd, Eq, Ord, Hash)]
pub struct Timestamp(u64);

impl Timestamp {
    pub const MAX: Self = Timestamp(u64::MAX);
    pub const MIN: Self = Timestamp(u64::MIN);

    /// Create a timestamp based on the provided `nanos`.
    #[inline]
    pub const fn from_nanos(nanos: u64) -> Self {
        Timestamp(nanos)
    }

    /// Returns a current timestamp.
    /// Use `pause`, `advance` and `resume` to control returned values
    /// if the `test-util` feature is enabled.
    pub fn now() -> Timestamp {
        clock::now()
    }

    #[inline]
    pub const fn as_nanos(self) -> u64 {
        self.0
    }

    /// ## Examples
    /// ```
    /// # use elfo_core::time::Timestamp;
    /// let ts1 = Timestamp::now();
    /// let ts2 = Timestamp::now();
    ///
    /// let delta = ts2.duration_since(ts1).unwrap_or_default();
    /// ```
    #[inline]
    pub fn duration_since(self, other: Timestamp) -> Option<Duration> {
        self.0.checked_sub(other.0).map(Duration::from_nanos)
    }
}

impl Add<Duration> for Timestamp {
    type Output = Self;

    #[inline]
    fn add(self, duration: Duration) -> Self {
        let duration_ns = duration.as_nanos() as u64;
        self.0
            .checked_add(duration_ns)
            .map(Timestamp)
            .expect("overflow when adding duration")
    }
}

impl Sub<Duration> for Timestamp {
    type Output = Self;

    #[inline]
    fn sub(self, duration: Duration) -> Self {
        let duration_ns = duration.as_nanos() as u64;
        self.0
            .checked_sub(duration_ns)
            .map(Timestamp)
            .expect("overflow when subtracting duration")
    }
}

#[cfg(not(feature = "test-util"))]
mod clock {
    use super::*;

    use std::time::UNIX_EPOCH;

    pub(super) fn now() -> Timestamp {
        let duration = UNIX_EPOCH.elapsed().expect("invalid system time");
        Timestamp(duration.as_nanos() as u64)
    }
}

#[cfg(feature = "test-util")]
mod clock {
    use super::*;

    use parking_lot::{const_mutex, Mutex};

    static ORIGIN: Mutex<Option<Instant>> = const_mutex(None);

    pub(super) fn now() -> Timestamp {
        let orig = *ORIGIN.lock().get_or_insert_with(Instant::now);
        Timestamp(orig.elapsed().as_nanos() as u64)
    }

    #[tokio::test]
    async fn it_works() {
        let ts1 = Timestamp::now();
        tokio::task::yield_now().await;
        let ts2 = Timestamp::now();
        super::pause();
        let ts3 = Timestamp::now();
        tokio::task::yield_now().await;
        let ts4 = Timestamp::now();
        super::advance(Duration::from_millis(3)).await;
        let ts5 = Timestamp::now();
        super::resume();
        let ts6 = Timestamp::now();

        assert_ne!(ts1, ts2);
        assert_ne!(ts2, ts3);
        assert_eq!(ts3, ts4);
        assert_ne!(ts4, ts5);
        assert_ne!(ts5, ts6);
    }
}
