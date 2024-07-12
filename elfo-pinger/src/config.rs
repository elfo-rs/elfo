//! Configuration for the pinger.
//!
//! Note: all types here are exported only for documentation purposes
//! and are not subject to stable guarantees. However, the config
//! structure (usually encoded in TOML) follows stable guarantees.

use std::time::Duration;

use serde::Deserialize;

/// The pinger's config.
///
/// # Example
/// ```toml
/// [system.pingers]
/// ping_interval = "30s"
/// ```
#[derive(Debug, Deserialize)]
pub struct Config {
    /// How often pingers should ping all other actors.
    ///
    /// `10s` by default.
    #[serde(with = "humantime_serde", default = "default_ping_interval")]
    pub ping_interval: Duration,
    /// How long to wait for a response before logging a warning.
    ///
    /// `5s` by default.
    #[serde(with = "humantime_serde", default = "default_warn_threshold")]
    pub warn_threshold: Duration,
}

fn default_ping_interval() -> Duration {
    Duration::from_secs(10)
}

fn default_warn_threshold() -> Duration {
    Duration::from_secs(5)
}
