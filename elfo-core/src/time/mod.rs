#[cfg(feature = "test-util")]
pub use tokio::time::{advance, pause, resume};
pub use tokio::time::{Duration, Instant};

pub use self::{interval::Interval, stopwatch::Stopwatch};

mod interval;
mod stopwatch;
