use std::{net::SocketAddr, ops::Deref, time::Duration};

use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub(crate) struct Config {
    /// The sink's type.
    pub(crate) sink: Sink,
    /// The address to expose for scraping.
    #[serde(alias = "address")]
    pub(crate) listen: SocketAddr,
    /// How long samples should be considered in summaries.
    #[serde(default)]
    pub(crate) retention: Retention,
    /// Quantiles to use for aggregating distribution metrics into a summary.
    #[serde(default = "default_quantiles")]
    pub(crate) quantiles: Vec<Quantile>,
    /// Labels that will be added to all metrics.
    #[serde(default)]
    pub(crate) global_labels: Vec<(String, String)>,
    /// The maximum time between compaction ticks.
    #[serde(with = "humantime_serde", default = "default_compaction_interval")]
    pub(crate) compaction_interval: Duration,
}

#[derive(Debug, PartialEq, Deserialize)]
pub(crate) enum Sink {
    #[serde(alias = "Prometheus")]
    OpenMetrics,
}

#[derive(Debug, PartialEq, Deserialize)]
pub(crate) enum Retention {
    Forever,
    ResetOnScrape,
    // TODO: `SlidingWindow`
}

impl Default for Retention {
    fn default() -> Self {
        Self::ResetOnScrape
    }
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(try_from = "f64")]
pub(crate) struct Quantile(f64);

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
