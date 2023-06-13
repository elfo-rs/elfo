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
}

impl Default for FilteringConfig {
    fn default() -> Self {
        Self {
            targets: Targets::new().with_default(LevelFilter::INFO),
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
        let targets = Targets::new()
            .with_default(LevelFilter::TRACE)
            .with_targets(
                config
                    .targets
                    .iter()
                    .map(|(target, target_config)| (target, target_config.max_level)),
            );

        self.inner
            .config
            .store(Arc::new(FilteringConfig { targets }));

        tracing::callsite::rebuild_interest_cache();
    }
}

impl<S: Subscriber> Layer<S> for FilteringLayer {
    fn register_callsite(&self, meta: &'static Metadata<'static>) -> Interest {
        let config = self.inner.config.load();
        if config.targets.would_enable(meta.target(), meta.level()) {
            // Not `::always()`, because actor can impose its own limits.
            Interest::sometimes()
        } else {
            // Won't be ever allowed by the `.targets`.
            Interest::never()
        }
    }

    fn enabled(&self, meta: &Metadata<'_>, cx: Context<'_, S>) -> bool {
        let config = self.inner.config.load();
        let targets_enabled = config.targets.enabled(meta, cx);

        scope::try_with(|scope| {
            let level = *meta.level();

            if !scope.permissions().is_logging_enabled(level) {
                return false;
            }

            match scope.logging().check(meta) {
                CheckResult::Passed => targets_enabled,
                CheckResult::NotInterested => false,
                CheckResult::Limited => {
                    stats::counter_per_level("elfo_limited_events_total", level);
                    false
                }
            }
        })
        .unwrap_or(targets_enabled)
    }

    // TODO: global max level and `max_level_hint()`.
}
