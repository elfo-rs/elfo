use tracing::{subscriber::Interest, Level, Metadata, Subscriber};
use tracing_subscriber::layer::{Context, Layer};

use elfo_core::scope;

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
    fn register_callsite(&self, _metadata: &'static Metadata<'static>) -> Interest {
        Interest::sometimes()
    }

    fn enabled(&self, metadata: &Metadata<'_>, _ctx: Context<'_, S>) -> bool {
        scope::try_with(|scope| {
            let level = *metadata.level();

            if !scope.permissions().is_logging_enabled(level) {
                return false;
            }

            if scope.logging_limiter().acquire() {
                true
            } else {
                stats::counter_per_level("elfo_limited_events_total", level);
                false
            }
        })
        // TODO: limit events outside the actor system?
        .unwrap_or_else(|| *metadata.level() <= self.other_max_level)
    }

    // TODO: implement `max_level_hint()`
}
