use std::sync::Arc;

use arc_swap::ArcSwap;
use tracing::{metadata::LevelFilter, subscriber::Interest, Metadata, Subscriber};
use tracing_subscriber::{
    filter::Targets,
    layer::{Context, Layer},
};

use elfo_core::{logging::_priv::CheckResult, scope};

use crate::stats;

struct FilteringConfig {
    targets: Targets,
    max_level: LevelFilter,
}

impl Default for FilteringConfig {
    fn default() -> Self {
        Self {
            targets: Targets::new().with_default(LevelFilter::INFO),
            max_level: LevelFilter::INFO,
        }
    }
}

struct Inner {
    config: ArcSwap<FilteringConfig>,
}

#[derive(Clone)]
pub struct FilteringLayer {
    inner: Arc<Inner>,
}

impl FilteringLayer {
    pub(crate) fn new() -> Self {
        Self {
            inner: Arc::new(Inner {
                config: ArcSwap::new(Arc::new(FilteringConfig::default())),
            }),
        }
    }

    pub(crate) fn configure(&self, config: &crate::config::Config) {
        let mut max_level = config.max_level;
        let targets = Targets::new().with_default(config.max_level).with_targets(
            config
                .targets
                .iter()
                // Remove entries that are equal to default anyway.
                .filter(|(_, target_config)| target_config.max_level != config.max_level)
                .map(|(target, target_config)| (target, target_config.max_level))
                // Calculate a total max level for cheap access.
                .inspect(|(_target, level)| max_level = max_level.max(*level)),
        );

        self.inner
            .config
            .store(Arc::new(FilteringConfig { max_level, targets }));

        tracing::callsite::rebuild_interest_cache();
    }
}

impl<S: Subscriber> Layer<S> for FilteringLayer {
    fn register_callsite(&self, meta: &'static Metadata<'static>) -> Interest {
        let config = self.inner.config.load();
        // Probably a cheaper check than `.would_enable()` below.
        if meta.level() > &config.max_level {
            // Won't be ever allowed by the `.targets`, because `.max_level` is max of all
            // their levels.
            Interest::never()
        } else if config.targets.would_enable(meta.target(), meta.level()) {
            // Not `::always()`, because actor could impose its own limits.
            Interest::sometimes()
        } else {
            // Won't be ever allowed by the `.targets`.
            Interest::never()
        }
    }

    fn enabled(&self, meta: &Metadata<'_>, cx: Context<'_, S>) -> bool {
        let passed_by_actor = scope::try_with(|scope| {
            let level = *meta.level();

            if !scope.permissions().is_logging_enabled(level) {
                return false;
            }

            match scope.logging().check(meta) {
                CheckResult::Passed => true,
                CheckResult::NotInterested => false,
                CheckResult::Limited => {
                    stats::counter_per_level("elfo_limited_events_total", level);
                    false
                }
            }
        });

        if passed_by_actor == Some(false) {
            return false;
        }

        self.inner.config.load().targets.enabled(meta, cx)
    }

    // REVIEW: that's a private API lol
    fn max_level_hint(&self) -> Option<LevelFilter> {
        Some(self.inner.config.load().max_level)
    }
}
