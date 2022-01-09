use std::time::Duration;

use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub(crate) struct Config {
    /// A path to a dump file or template:
    /// * `dumps/all.dump` - one file.
    /// * `dumps/{class}.dump` - file per class.
    path: String,
    #[serde(with = "humantime_serde", default = "default_interval")]
    pub(crate) interval: Duration,
    #[serde(default = "default_registry_capacity")]
    pub(crate) registry_capacity: usize,
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
