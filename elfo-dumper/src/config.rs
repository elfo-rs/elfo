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
    pub(crate) interval: Duration,
    /// The maximum number of dumps in memory per class.
    /// 3_000_000 by default.
    #[serde(default = "default_registry_capacity")]
    pub(crate) registry_capacity: usize,
    /// The allowed size of a serialized dump.
    /// If exceeded, the dump is lost with limited logging.
    /// 64KiB by default.
    #[serde(default = "default_max_dump_size", with = "bytesize_serde")]
    pub(crate) max_dump_size: ByteSize,
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

fn default_max_dump_size() -> ByteSize {
    ByteSize::kib(64)
}
