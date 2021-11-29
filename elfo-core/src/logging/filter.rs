use tracing::{level_filters::LevelFilter, Metadata, Subscriber};
use tracing_subscriber::{filter::Targets, layer::Context, Layer};

use super::config::LoggingConfig;

pub(super) struct LogFilter {
    max_level: LevelFilter,
    targets: Option<Targets>,
}

impl Default for LogFilter {
    fn default() -> Self {
        Self {
            max_level: LevelFilter::OFF,
            targets: None,
        }
    }
}

impl LogFilter {
    pub(super) fn new(config: &LoggingConfig) -> Self {
        let mut max_level = config.max_level;

        let use_targets = config.targets.values().any(|c| c.max_level != max_level);
        let targets = if use_targets {
            let targets = Targets::new().with_default(max_level).with_targets(
                config
                    .targets
                    .iter()
                    .filter(|(_, target_config)| target_config.max_level != config.max_level)
                    .map(|(target, target_config)| (target, target_config.max_level))
                    .inspect(|(_, level)| max_level = max_level.max(*level)),
            );

            Some(targets)
        } else {
            None
        };

        Self { max_level, targets }
    }

    pub(super) fn max_level_hint(&self) -> LevelFilter {
        self.max_level
    }

    pub(super) fn enabled(&self, meta: &Metadata<'_>, cx: Context<'_, impl Subscriber>) -> bool {
        if let Some(targets) = &self.targets {
            targets.enabled(meta, cx)
        } else {
            *meta.level() <= self.max_level
        }
    }
}

#[test]
fn level_ord() {
    use tracing::Level;

    assert!(Level::ERROR > LevelFilter::OFF);
    assert!(Level::INFO > LevelFilter::ERROR);
    assert_eq!(Level::INFO, LevelFilter::INFO);
    assert!(Level::INFO < LevelFilter::DEBUG);

    assert!(LevelFilter::ERROR > LevelFilter::OFF);
    assert!(LevelFilter::INFO > LevelFilter::ERROR);
    assert!(LevelFilter::INFO < LevelFilter::DEBUG);
}
