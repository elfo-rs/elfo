use std::time::Duration;

use tokio::time::Instant;

pub use self::{delay::Delay, interval::Interval};

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
