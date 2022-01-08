use std::{path::PathBuf, time::Duration};

use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub(crate) struct Config {
    pub(crate) path: PathBuf,
    #[serde(with = "humantime_serde", default = "default_interval")]
    pub(crate) interval: Duration,
    #[serde(default = "default_registry_capacity")]
    pub(crate) registry_capacity: usize,
}

fn default_interval() -> Duration {
    Duration::from_millis(500)
}

fn default_registry_capacity() -> usize {
    3_000_000
}
