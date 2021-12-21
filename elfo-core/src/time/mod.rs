use std::time::SystemTime;

pub use self::{interval::Interval, stopwatch::Stopwatch};

pub(crate) use r#impl::*;

mod interval;
mod stopwatch;

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
