use std::time::Duration;

use bytesize::ByteSize;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub(crate) struct Config {
    /// A path to a dump file or template:
    /// * `dumps/all.dump` - one file.
    /// * `dumps/{class}.dump` - file per class.
    path: String,
    /// How often dumpers should flush dumps to file.
    /// 500ms by default.
    #[serde(with = "humantime_serde", default = "default_interval")]
    pub interval: Duration,
    #[serde(with = "humantime_serde", default = "default_warn_cooldown")]
    pub warn_cooldown: Duration,
    /// The maximum number of dumps in memory per class.
    /// 3_000_000 by default.
    #[serde(default = "default_registry_capacity")]
    pub(crate) registry_capacity: usize,
    /// The allowed size of a serialized dump.
    /// If exceeded, the dump is lost with limited logging.
    /// 64KiB by default.
    // TODO: deprecated
    #[serde(default = "default_max_dump_size")]
    pub max_dump_size: usize,
    #[serde(default)]
    pub rules: Vec<Rule>,
}

// TODO
#[derive(Debug, Default, Clone, Deserialize, PartialEq, Eq)]
pub(crate) struct Rule {
    // Matchers.
    pub(crate) class: Option<String>,
    pub(crate) protocol: Option<String>,
    pub(crate) message: Option<String>,
    // Params.
    pub max_size: Option<ByteSize>,
    pub on_overflow: Option<OnOverflow>,
    pub log_on_overflow: Option<LogLevel>,
    pub log_on_failure: Option<LogLevel>,
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

fn default_warn_cooldown() -> Duration {
    Duration::from_secs(60)
}

fn default_registry_capacity() -> usize {
    3_000_000
}

fn default_max_dump_size() -> usize {
    64 * 1024
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
    Off,
}
