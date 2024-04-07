//! Contains the protocol to interact with the telemeter.

use std::sync::Arc;

use fxhash::FxHashMap;
use metrics::Key;
use metrics_util::Summary; // TODO: avoid this dependency

use elfo_core::{message, ActorMeta, Local};

#[message(ret = Rendered)]
pub(crate) struct Render;

#[message]
pub(crate) struct Rendered(#[serde(serialize_with = "elfo_core::dumping::hide")] pub(crate) String);

#[message]
pub(crate) struct ServerFailed(pub(crate) String);

/// A command to get actual snapshot of all metrics.
/// The response is restricted to be local only for now.
#[message(ret = Local<Arc<Snapshot>>)]
#[non_exhaustive]
pub(crate) struct GetSnapshot;

pub(crate) type GaugeEpoch = u64;

/// Actual values of all metrics.
#[derive(Default, Clone)]
pub struct Snapshot {
    /// Metrics ouside the actor system.
    pub global: Metrics,
    /// Metrics aggregated per group.
    pub groupwise: FxHashMap<String, Metrics>,
    /// Metrics aggregated per actor.
    pub actorwise: FxHashMap<Arc<ActorMeta>, Metrics>,
}

impl Snapshot {
    pub(crate) fn histograms_mut(&mut self) -> impl Iterator<Item = &mut Distribution> {
        let global = self.global.histograms.values_mut();

        let per_group = self
            .groupwise
            .values_mut()
            .flat_map(|m| m.histograms.values_mut());

        let per_actor = self
            .actorwise
            .values_mut()
            .flat_map(|m| m.histograms.values_mut());

        global.chain(per_group).chain(per_actor)
    }
}

/// Actual values of all metrics grouped by a metric type.
#[derive(Default, Clone)]
pub struct Metrics {
    /// Monotonically increasing counters.
    pub counters: FxHashMap<Key, u64>,
    /// Numerical values that can arbitrarily go up and down.
    pub gauges: FxHashMap<Key, (f64, GaugeEpoch)>,
    /// Summaries of samples, used to calculate of quantiles.
    pub histograms: FxHashMap<Key, Distribution>,
}

/// Summaries of samples, used to calculate of quantiles.
#[derive(Clone)]
pub struct Distribution {
    summary: Arc<Summary>,
    sum: f64,
    count: usize,
}

impl Default for Distribution {
    fn default() -> Self {
        let summary = Summary::with_defaults();
        Self {
            summary: Arc::new(summary),
            sum: 0.0,
            count: 0,
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
        self.summary.quantile(q).filter(|f| f.is_finite())
    }

    /// Gets the minimum value this summary has seen so far.
    #[inline]
    pub fn min(&self) -> Option<f64> {
        Some(self.summary.min()).filter(|f| f.is_finite())
    }

    /// Gets the maximum value this summary has seen so far.
    #[inline]
    pub fn max(&self) -> Option<f64> {
        Some(self.summary.max()).filter(|f| f.is_finite())
    }

    /// Gets the number of samples in this distribution.
    #[inline]
    pub fn count(&self) -> usize {
        self.count
    }

    /// Gets the maximum value this summary has seen so far.
    #[inline]
    pub fn sum(&self) -> f64 {
        self.sum
    }

    pub(crate) fn reset(&mut self) {
        self.summary = Arc::new(Summary::with_defaults());
        // Do not reset `count` and `sum` fields.
    }

    pub(crate) fn record_samples(&mut self, samples: &[f64]) {
        let summary = Arc::make_mut(&mut self.summary);
        for sample in samples {
            summary.add(*sample);
            self.sum += *sample;
            self.count += 1;
        }
    }

    pub(crate) fn estimated_size(&self) -> usize {
        self.summary.estimated_size() + std::mem::size_of::<Self>()
    }
}
