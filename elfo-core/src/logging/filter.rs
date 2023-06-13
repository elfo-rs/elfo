use tracing::{level_filters::LevelFilter, Metadata};

use super::config::LoggingConfig;

pub(super) struct LogFilter {
    max_level: LevelFilter,
}

impl Default for LogFilter {
    fn default() -> Self {
        Self {
            max_level: LevelFilter::OFF,
        }
    }
}

impl LogFilter {
    pub(super) fn new(config: &LoggingConfig) -> Self {
        Self {
            max_level: config.max_level,
        }
    }

    pub(super) fn max_level_hint(&self) -> LevelFilter {
        self.max_level
    }

    pub(super) fn enabled(&self, meta: &Metadata<'_>) -> bool {
        *meta.level() <= self.max_level
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
