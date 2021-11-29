use std::sync::Arc;

use arc_swap::ArcSwap;
use tracing::{level_filters::LevelFilter, Metadata, Subscriber};
use tracing_subscriber::layer::Context;

use elfo_utils::RateLimiter;

use super::{config::LoggingConfig, filter::LogFilter};

#[derive(Default)]
pub struct LoggingControl {
    filter: ArcSwap<LogFilter>,
    limiter: RateLimiter,
}

impl LoggingControl {
    pub(crate) fn configure(&self, config: &LoggingConfig) {
        self.filter.store(Arc::new(LogFilter::new(config)));
        self.limiter.configure(config.max_rate);
    }

    pub(crate) fn max_level_hint(&self) -> LevelFilter {
        self.filter.load().max_level_hint()
    }

    pub fn check(&self, meta: &Metadata<'_>, cx: Context<'_, impl Subscriber>) -> CheckResult {
        if !self.filter.load().enabled(meta, cx) {
            return CheckResult::NotInterested;
        }

        if self.limiter.acquire() {
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
