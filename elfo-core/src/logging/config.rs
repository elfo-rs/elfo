use serde::{Deserialize, Deserializer};
use tracing::level_filters::LevelFilter;

#[derive(Deserialize)]
#[serde(default)]
pub(crate) struct LoggingConfig {
    #[serde(deserialize_with = "deserialize_level_filter")]
    pub(crate) max_level: LevelFilter,
    pub(crate) max_rate_per_level: u64,
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
