//! Configuration for the telemeter.
//!
//! Note: all types here are exported only for documentation purposes
//! and are not subject to stable guarantees. However, the config
//! structure (usually encoded in TOML) follows stable guarantees.

use std::{net::SocketAddr, ops::Deref, time::Duration};

use serde::Deserialize;

/// Telemeter configuration.
///
/// # Example
/// ```toml
/// [system.telemeters]
/// sink = "OpenMetrics"
/// listen = "0.0.0.0:9042"
/// ```
#[derive(Debug, Deserialize)]
pub struct Config {
    /// The sink's type.
    pub sink: Sink,
    /// The address to expose for scraping.
    #[serde(alias = "address")]
    pub listen: SocketAddr,
    /// How long samples should be considered in summaries.
    #[serde(default)]
    pub retention: Retention,
    /// Quantiles to use for aggregating distribution metrics into a summary.
    ///
    /// The default quantiles are `[0.75, 0.9, 0.95, 0.99]`.
    #[serde(default = "default_quantiles")]
    pub quantiles: Vec<Quantile>,
    /// Labels that will be added to all metrics.
    #[serde(default)]
    pub global_labels: Vec<(String, String)>,
    /// The maximum time between compaction ticks.
    ///
    /// `1.1s` by default.
    #[serde(with = "humantime_serde", default = "default_compaction_interval")]
    pub compaction_interval: Duration,
}

/// Sink for the telemeter output.
#[derive(Debug, PartialEq, Deserialize)]
pub enum Sink {
    /// Expose metrics in the OpenMetrics/Prometheus format.
    #[serde(alias = "Prometheus")]
    OpenMetrics,
}

/// Histogram/summary retention policy.
#[derive(Debug, PartialEq, Deserialize)]
pub enum Retention {
    /// Keep all samples forever.
    Forever,
    /// Reset all samples on each scrape.
    ResetOnScrape,
    // TODO: `SlidingWindow`
}

impl Default for Retention {
    fn default() -> Self {
        Self::ResetOnScrape
    }
}

/// A quantile to use for aggregating distribution metrics into a summary
/// with the `quantile` label. Must be in the range [0.0, 1.0].
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(try_from = "f64")]
pub struct Quantile(f64);

impl Deref for Quantile {
    type Target = f64;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl TryFrom<f64> for Quantile {
    type Error = String;

    fn try_from(value: f64) -> Result<Self, Self::Error> {
        (0.0..=1.0)
            .contains(&value)
            .then_some(Self(value))
            .ok_or_else(|| format!("invalid quantile {value}, must be in the range [0.0, 1.0]"))
    }
}

fn default_quantiles() -> Vec<Quantile> {
    [0.75, 0.9, 0.95, 0.99].into_iter().map(Quantile).collect()
}

fn default_compaction_interval() -> Duration {
    // 1m, 30s, 15s, 10s are often used values of prometheus's `scrape_interval`.
    // 1.1s is a good value that splits the scrape interval uniformly enough.
    Duration::from_millis(1100)
}
