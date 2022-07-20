use std::sync::Arc;

use arc_swap::ArcSwap;
use tracing::{level_filters::LevelFilter, Level, Metadata, Subscriber};
use tracing_subscriber::layer::Context;

use elfo_utils::{CachePadded, RateLimit, RateLimiter};

use super::{config::LoggingConfig, filter::LogFilter};

#[derive(Default)]
#[stability::unstable]
pub struct LoggingControl {
    filter: ArcSwap<LogFilter>,
    limiters: [CachePadded<RateLimiter>; 5],
}

impl LoggingControl {
    pub(crate) fn configure(&self, config: &LoggingConfig) {
        self.filter.store(Arc::new(LogFilter::new(config)));

        for limiter in &self.limiters {
            limiter.configure(RateLimit::Rps(config.max_rate_per_level));
        }
    }

    pub(crate) fn max_level_hint(&self) -> LevelFilter {
        self.filter.load().max_level_hint()
    }

    pub fn check(&self, meta: &Metadata<'_>, cx: Context<'_, impl Subscriber>) -> CheckResult {
        if !self.filter.load().enabled(meta, cx) {
            return CheckResult::NotInterested;
        }

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
