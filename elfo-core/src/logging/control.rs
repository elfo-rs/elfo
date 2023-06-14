use tracing::{Level, Metadata};

use elfo_utils::{CachePadded, RateLimit, RateLimiter};

use super::config::LoggingConfig;

#[derive(Default)]
#[stability::unstable]
pub struct LoggingControl {
    limiters: [CachePadded<RateLimiter>; 5],
}

impl LoggingControl {
    pub(crate) fn configure(&self, config: &LoggingConfig) {
        for limiter in &self.limiters {
            limiter.configure(RateLimit::Rps(config.max_rate_per_level));
        }
    }

    pub fn check(&self, meta: &Metadata<'_>) -> CheckResult {
        let limiter = &self.limiters[log_level_to_value(*meta.level())];
        if limiter.acquire() {
            CheckResult::Passed
        } else {
            CheckResult::Limited
        }
    }
}

pub enum CheckResult {
    Passed,
    NotInterested,
    Limited,
}

fn log_level_to_value(level: Level) -> usize {
    // The compiler should optimize this expression because `tracing::Level` is
    // based on the same values internally.
    match level {
        Level::TRACE => 0,
        Level::DEBUG => 1,
        Level::INFO => 2,
        Level::WARN => 3,
        Level::ERROR => 4,
    }
}
