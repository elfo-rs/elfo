use std::net::SocketAddr;

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
    // TODO: add `global_labels`?
}

#[derive(Debug, PartialEq, Deserialize)]
pub(crate) enum Sink {
    Prometheus,
}

fn default_quantiles() -> Vec<f64> {
    vec![0.5, 0.75, 0.9, 0.95, 0.99]
}
