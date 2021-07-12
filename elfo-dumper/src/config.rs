use std::{path::PathBuf, time::Duration};

use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub(crate) struct Config {
    pub(crate) path: PathBuf,
    #[serde(with = "humantime_serde", default = "default_interval")]
    pub(crate) interval: Duration,
}

fn default_interval() -> Duration {
    Duration::from_millis(250)
}
