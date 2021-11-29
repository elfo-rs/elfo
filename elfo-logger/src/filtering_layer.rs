use tracing::{subscriber::Interest, Level, Metadata, Subscriber};
use tracing_subscriber::layer::{Context, Layer};

use elfo_core::{logging::_priv::CheckResult, scope};

use crate::stats;

#[non_exhaustive]
pub(crate) struct FilteringLayer {
    other_max_level: Level,
}

impl FilteringLayer {
    pub(crate) fn new(other_max_level: Level) -> Self {
        Self { other_max_level }
    }
}

impl<S: Subscriber> Layer<S> for FilteringLayer {
    fn register_callsite(&self, _meta: &'static Metadata<'static>) -> Interest {
        Interest::sometimes()
    }

    fn enabled(&self, meta: &Metadata<'_>, cx: Context<'_, S>) -> bool {
        scope::try_with(|scope| {
            let level = *meta.level();

            if !scope.permissions().is_logging_enabled(level) {
                return false;
            }

            match scope.logging().check(meta, cx) {
                CheckResult::Passed => true,
                CheckResult::NotInterested => false,
                CheckResult::Limited => {
                    stats::counter_per_level("elfo_limited_events_total", level);
                    false
                }
            }
        })
        // TODO: limit events outside the actor system?
        .unwrap_or_else(|| *meta.level() <= self.other_max_level)
    }

    // TODO: implement `max_level_hint()`
}
