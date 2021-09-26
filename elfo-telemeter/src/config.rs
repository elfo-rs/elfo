use std::{net::SocketAddr, time::Duration};

use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub(crate) struct Config {
    /// The sink's type.
    pub(crate) sink: Sink,
    /// The address to expose for scraping.
    pub(crate) address: SocketAddr,
    /// Quantiles to use for aggregating distribution metrics into a summary.
    #[serde(default = "default_quantiles")]
    pub(crate) quantiles: Vec<f64>,
    /// Labels that will be added to all metrics.
    #[serde(default)]
    pub(crate) global_labels: Vec<(String, String)>,
    /// The maximum time between compaction ticks.
    #[serde(with = "humantime_serde", default = "default_compaction_interval")]
    pub(crate) compaction_interval: Duration,
}

#[derive(Debug, PartialEq, Deserialize)]
pub(crate) enum Sink {
    Prometheus,
}

fn default_quantiles() -> Vec<f64> {
    vec![0.75, 0.9, 0.95, 0.99]
}

fn default_compaction_interval() -> Duration {
    // 1m, 30s, 15s, 10s are often used values of prometheus's `scrape_interval`.
    // 4.1s is a good value that splits the scrape interval uniformly enough.
    Duration::from_millis(4100)
}
