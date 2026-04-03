use std::sync::Arc;
#[cfg(feature = "tracing-log")]
use std::sync::OnceLock;

use arc_swap::ArcSwap;
use fxhash::FxHashMap;
use tracing::{Metadata, Subscriber, metadata::LevelFilter, subscriber::Interest};
use tracing_subscriber::{filter::Targets, layer};

use elfo_core::{logging::CheckResult, scope};

use crate::{config::LoggingTargetConfig, stats};

#[derive(PartialEq)]
struct ScopeFilterConfig {
    targets: Targets,
}

impl Default for ScopeFilterConfig {
    fn default() -> Self {
        Self {
            targets: Targets::new().with_default(LevelFilter::TRACE),
        }
    }
}

struct Inner {
    config: ArcSwap<ScopeFilterConfig>,
    #[cfg(feature = "tracing-log")]
    log_metadata_name: OnceLock<&'static str>,
}

/// A tracing layer that filters events based on the current scope's permissions
/// and the configured targets.
///
/// For details see:
/// * [Actor configuration] for per-group logging levels and rate limits.
/// * [The logger's configuration] for limiting targets.
///
/// It implements both `Layer` and `Filter` traits, so it can be used either
/// as a layer of a subscriber or as a filter of another layer.
///
/// [Actor configuration]: elfo_core::config::system::logging::LoggingConfig
/// [The logger's configuration]: crate::config::Config::targets
pub struct ScopeFilter {
    inner: Arc<Inner>,
}

impl ScopeFilter {
    pub(crate) fn new() -> Self {
        Self {
            inner: Arc::new(Inner {
                config: ArcSwap::new(Arc::new(ScopeFilterConfig::default())),
                #[cfg(feature = "tracing-log")]
                log_metadata_name: OnceLock::new(),
            }),
        }
    }

    // I'm not sure I would like to stabilize `Clone` for now.
    pub(crate) fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
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

        let config = Arc::new(ScopeFilterConfig { targets });
        let old_config = self.inner.config.swap(Arc::clone(&config));
        if config != old_config {
            tracing::callsite::rebuild_interest_cache();
        }
    }

    fn interested(&self, meta: &Metadata<'_>) -> Interest {
        let config = self.inner.config.load();
        if config.targets.would_enable(meta.target(), meta.level()) {
            // Not `::always()`, because actor can impose its own limits.
            Interest::sometimes()
        } else {
            // Won't be ever allowed by the `.targets`.
            Interest::never()
        }
    }

    fn enabled(&self, meta: &Metadata<'_>) -> bool {
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
}

impl<S: Subscriber> layer::Layer<S> for ScopeFilter {
    fn register_callsite(&self, meta: &'static Metadata<'static>) -> Interest {
        self.interested(meta)
    }

    fn enabled(&self, meta: &Metadata<'_>, _cx: layer::Context<'_, S>) -> bool {
        self.enabled(meta)
    }

    // TODO: global max level and `max_level_hint()`.
}

// TODO: check either `rebuild_interest_cache` work here or not.
impl<S> layer::Filter<S> for ScopeFilter {
    fn callsite_enabled(&self, meta: &'static Metadata<'static>) -> Interest {
        self.interested(meta)
    }

    fn enabled(&self, meta: &Metadata<'_>, _cx: &layer::Context<'_, S>) -> bool {
        // Even though we are implementing `callsite_enabled`, we must still provide a
        // working implementation of `enabled`, as returning `Interest::always()` or
        // `Interest::never()` will *allow* caching, but will not *guarantee* it.
        // Other filters may still return `Interest::sometimes()`, so we may be
        // asked again in `enabled`.
        !self.interested(meta).is_never() && self.enabled(meta)
    }

    // TODO: global max level and `max_level_hint()`.
}
