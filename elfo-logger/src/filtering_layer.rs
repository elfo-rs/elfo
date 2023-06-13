use std::sync::Arc;

use arc_swap::ArcSwap;
use tracing::{metadata::LevelFilter, subscriber::Interest, Metadata, Subscriber};
use tracing_subscriber::{
    filter::Targets,
    layer::{Context, Layer},
};

use elfo_core::{logging::_priv::CheckResult, scope};

use crate::stats;

#[derive(Debug)]
pub(crate) struct FilteringConfig {
    pub(crate) targets: Targets,
    pub(crate) max_level: LevelFilter,
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
                .filter(|(_, target_config)| target_config.max_level != config.max_level)
                .map(|(target, target_config)| (target, target_config.max_level))
                .inspect(|(_target, level)| max_level = max_level.max(*level)),
        );
        self.inner
            .config
            .store(Arc::new(FilteringConfig { max_level, targets }));
    }
}

impl<S: Subscriber> Layer<S> for FilteringLayer {
    fn register_callsite(&self, _meta: &'static Metadata<'static>) -> Interest {
        Interest::sometimes()
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
        })
        .unwrap_or(true);

        if !passed_by_actor {
            return false;
        }

        let config = self.inner.config.load();
        config.targets.enabled(meta, cx)
    }

    fn max_level_hint(&self) -> Option<LevelFilter> {
        Some(self.inner.config.load().max_level)
    }
}
