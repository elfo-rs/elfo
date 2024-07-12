//! [Config].
//!
//! [Config]: LoggingConfig

use serde::{Deserialize, Deserializer};
use tracing::level_filters::LevelFilter;

/// Logging configuration.
///
/// # Example
/// ```toml
/// [some_group]
/// system.logging.max_level = "Warn"
/// system.logging.max_rate_per_level = 1_000
/// ```
#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct LoggingConfig {
    /// Maximum level of logging.
    ///
    /// `Info` by default.
    #[serde(deserialize_with = "deserialize_level_filter")]
    pub max_level: LevelFilter,
    /// Maximum rate of logging per level.
    ///
    /// `1_000` by default.
    pub max_rate_per_level: u64,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            max_level: LevelFilter::INFO,
            max_rate_per_level: 1000,
        }
    }
}

fn deserialize_level_filter<'de, D>(deserializer: D) -> Result<LevelFilter, D::Error>
where
    D: Deserializer<'de>,
{
    use PrettyLevelFilter::*;

    #[derive(Deserialize)]
    pub(crate) enum PrettyLevelFilter {
        Trace,
        Debug,
        Info,
        Warn,
        Error,
        Off,
    }

    let pretty = PrettyLevelFilter::deserialize(deserializer)?;

    Ok(match pretty {
        Trace => LevelFilter::TRACE,
        Debug => LevelFilter::DEBUG,
        Info => LevelFilter::INFO,
        Warn => LevelFilter::WARN,
        Error => LevelFilter::ERROR,
        Off => LevelFilter::OFF,
    })
}
