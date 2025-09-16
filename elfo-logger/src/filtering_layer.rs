use std::sync::Arc;
#[cfg(feature = "tracing-log")]
use std::sync::OnceLock;

use arc_swap::ArcSwap;
use fxhash::FxHashMap;
use tracing::{metadata::LevelFilter, subscriber::Interest, Metadata, Subscriber};
use tracing_subscriber::{
    filter::Targets,
    layer::{Context, Layer},
};

use elfo_core::{logging::_priv::CheckResult, scope};

use crate::{config::LoggingTargetConfig, stats};

#[derive(PartialEq)]
struct FilteringConfig {
    targets: Targets,
}

impl Default for FilteringConfig {
    fn default() -> Self {
        Self {
            targets: Targets::new().with_default(LevelFilter::TRACE),
        }
    }
}

struct Inner {
    config: ArcSwap<FilteringConfig>,
    #[cfg(feature = "tracing-log")]
    log_metadata_name: OnceLock<&'static str>,
}

#[derive(Clone)]
pub(crate) struct FilteringLayer {
    inner: Arc<Inner>,
}

impl FilteringLayer {
    pub(crate) fn new() -> Self {
        Self {
            inner: Arc::new(Inner {
                config: ArcSwap::new(Arc::new(FilteringConfig::default())),
                #[cfg(feature = "tracing-log")]
                log_metadata_name: OnceLock::new(),
            }),
        }
    }

    pub(crate) fn configure(&self, targets: &FxHashMap<String, LoggingTargetConfig>) {
        let targets = Targets::new()
            .with_default(LevelFilter::TRACE)
            .with_targets(
                targets
                    .iter()
                    .map(|(target, target_config)| (target, target_config.max_level)),
            );

        let config = Arc::new(FilteringConfig { targets });
        let old_config = self.inner.config.swap(Arc::clone(&config));
        if config != old_config {
            tracing::callsite::rebuild_interest_cache();
        }
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

    fn enabled(&self, meta: &Metadata<'_>, _cx: Context<'_, S>) -> bool {
        // We don't need to recheck `.targets` here, because `.register_callsite()`
        // would already eliminate logs that would be filtered by it.
        let level = *meta.level();

        #[cfg(feature = "tracing-log")]
        {
            // `tracing-log` doesn't call `.register_callsite()`, so we have to recheck
            // `.targets` in this one case.
            let name = meta.name();
            if let Some(&log_name) = self.inner.log_metadata_name.get() {
                // We've already got logs from `tracing-log`, just compare str
                // pointers for performance.
                if std::ptr::eq(log_name, name) {
                    let config = self.inner.config.load();
                    if !config.targets.would_enable(meta.target(), meta.level()) {
                        return false;
                    }
                }
            } else if name == "log record" {
                let config = self.inner.config.load();
                if !config.targets.would_enable(meta.target(), meta.level()) {
                    return false;
                }
                // That's "initialized tracing_log adapter" to set `log_metadata_name`, just
                // ignore it. Any actual logs from `elfo_logger` would use `tracing`.
                if meta.target() == "elfo_logger" {
                    self.inner.log_metadata_name.set(name).ok();
                    return false;
                }
            }
        }

        scope::try_with(|scope| {
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
        // `INFO` is a global cap for non-actor logs.
        .unwrap_or(level <= LevelFilter::INFO)
    }

    // TODO: global max level and `max_level_hint()`.
}
