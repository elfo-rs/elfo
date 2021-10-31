//! Contains the protocol to interact with the telemeter.

use std::sync::Arc;

use fxhash::FxHashMap;
use metrics::Key;
use metrics_util::Summary;

use elfo_core::{ActorMeta, Local};
use elfo_macros::message;

/// A command to get actual snapshot of all metrics.
/// The response is restricted to be local only for now.
#[message(ret = Local<Arc<Snapshot>>, elfo = elfo_core)]
pub struct GetSnapshot;

/// Actual values of all metrics.
#[derive(Default, Clone)]
pub struct Snapshot {
    /// Metrics aggregated per group.
    pub per_group: FxHashMap<String, Metrics>,
    /// Metrics aggregated per actor.
    pub per_actor: FxHashMap<Arc<ActorMeta>, Metrics>,
}

/// Actual values of all metrics grouped by a metric type.
#[derive(Default, Clone)]
pub struct Metrics {
    /// Monotonically increasing counters.
    pub counters: FxHashMap<Key, u64>,
    /// Numerical values that can arbitrarily go up and down.
    pub gauges: FxHashMap<Key, f64>,
    /// Summaries of samples, used to calculate of quantiles.
    pub distributions: FxHashMap<Key, Distribution>,
}

/// Summaries of samples, used to calculate of quantiles.
#[derive(Clone)]
pub struct Distribution {
    summary: Arc<Summary>,
    sum: f64,
}

impl Default for Distribution {
    fn default() -> Self {
        let summary = Summary::with_defaults();
        Self {
            summary: Arc::new(summary),
            sum: 0.0,
        }
    }
}

impl Distribution {
    /// Gets the estimated value at the given quantile.
    ///
    /// If the data is empty, or if the quantile is less than 0.0 or greater
    /// than 1.0, then the result will be `None`.
    #[inline]
    pub fn quantile(&self, q: f64) -> Option<f64> {
        self.summary.quantile(q)
    }

    /// Gets the minimum value this summary has seen so far.
    #[inline]
    pub fn min(&self) -> f64 {
        self.summary.min()
    }

    /// Gets the maximum value this summary has seen so far.
    #[inline]
    pub fn max(&self) -> f64 {
        self.summary.max()
    }

    /// Gets the number of samples in this distribution.
    #[inline]
    pub fn count(&self) -> usize {
        self.summary.count()
    }

    /// Gets the maximum value this summary has seen so far.
    #[inline]
    pub fn sum(&self) -> f64 {
        self.sum
    }

    pub(crate) fn record_samples(&mut self, samples: &[f64]) {
        let summary = Arc::make_mut(&mut self.summary);
        for sample in samples {
            summary.add(*sample);
            self.sum += *sample;
        }
    }
}
