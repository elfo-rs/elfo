use std::time::Duration;

use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub(crate) struct Config {
    #[serde(with = "humantime_serde", default = "default_ping_interval")]
    pub(crate) ping_interval: Duration,
    #[serde(with = "humantime_serde", default = "default_warn_threshold")]
    pub(crate) warn_threshold: Duration,
}

fn default_ping_interval() -> Duration {
    Duration::from_secs(1)
}

fn default_warn_threshold() -> Duration {
    Duration::from_secs(5)
}
