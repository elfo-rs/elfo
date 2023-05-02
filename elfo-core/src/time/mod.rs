use std::time::{Duration, SystemTime};

use tokio::time::Instant;

pub use self::{delay::Delay, interval::Interval};

pub(crate) use r#impl::*;

mod delay;
mod interval;

fn far_future() -> Instant {
    // Copied from `tokio`.
    // Roughly 30 years from now.
    // API does not provide a way to obtain max `Instant`
    // or convert specific date in the future to instant.
    // 1000 years overflows on macOS, 100 years overflows on FreeBSD.
    Instant::now() + Duration::from_secs(86400 * 365 * 30)
}

#[cfg(test)]
pub(crate) mod r#impl {
    use std::{cell::Cell, time::Duration};

    use super::*;

    thread_local! {
        pub(crate) static NOW: Cell<Duration> = Default::default();
    }

    pub(crate) fn advance(duration: Duration) {
        NOW.with(|now| now.set(now.get() + duration))
    }

    pub(crate) fn now() -> SystemTime {
        SystemTime::UNIX_EPOCH + NOW.with(Cell::get)
    }
}

#[cfg(not(test))]
pub(crate) mod r#impl {
    use super::*;

    #[inline]
    pub(crate) fn now() -> SystemTime {
        SystemTime::now()
    }
}
