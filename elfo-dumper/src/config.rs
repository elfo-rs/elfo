use std::time::Duration;

use serde::{Deserialize, Deserializer};
use tracing::level_filters::LevelFilter;

#[derive(Debug, Deserialize)]
pub(crate) struct Config {
    /// A path to a dump file or template:
    /// * `dumps/all.dump` - one file.
    /// * `dumps/{class}.dump` - file per class.
    path: String,
    /// How often dumpers should flush dumps to file.
    /// 500ms by default.
    #[serde(with = "humantime_serde", default = "default_interval")]
    pub(crate) interval: Duration,
    /// The maximum number of dumps in memory per class.
    /// 3_000_000 by default.
    #[serde(default = "default_registry_capacity")]
    pub(crate) registry_capacity: usize,
    /// The allowed size of a serialized dump.
    /// If exceeded, the dump is lost with limited logging.
    /// 64KiB by default.
    // TODO: deprecated
    #[serde(default = "default_max_dump_size")]
    pub(crate) max_dump_size: usize,
    // TODO
    pub(crate) rules: Vec<Rule>,
}

// TODO
#[derive(Debug, Default, Clone, Deserialize, PartialEq, Eq)]
pub(crate) struct Rule {
    // Matchers.
    pub(crate) class: Option<String>,
    pub(crate) protocol: Option<String>,
    pub(crate) message: Option<String>,
    // Params.
    pub(crate) max_size: Option<usize>,
    pub(crate) on_overflow: Option<OnOverflow>,
    #[serde(deserialize_with = "deserialize_level_filter")]
    pub(crate) on_overflow_log: Option<LevelFilter>,
    #[serde(deserialize_with = "deserialize_level_filter")]
    pub(crate) on_failure_log: Option<LevelFilter>,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
pub(crate) enum OnOverflow {
    Skip,
    Truncate,
}

impl Config {
    pub(crate) fn path(&self, class: &str) -> String {
        self.path.replace("{class}", class)
    }
}

fn default_interval() -> Duration {
    Duration::from_millis(500)
}

fn default_registry_capacity() -> usize {
    3_000_000
}

fn default_max_dump_size() -> usize {
    64 * 1024
}

// TODO: deduplicate with `elfo-core/logging/config`.
fn deserialize_level_filter<'de, D>(deserializer: D) -> Result<Option<LevelFilter>, D::Error>
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

    Option::<PrettyLevelFilter>::deserialize(deserializer)?
        .map(|pretty| match pretty {
            Trace => LevelFilter::TRACE,
            Debug => LevelFilter::DEBUG,
            Info => LevelFilter::INFO,
            Warn => LevelFilter::WARN,
            Error => LevelFilter::ERROR,
            Off => LevelFilter::OFF,
        })
        .map(Ok)
        .transpose()
}
