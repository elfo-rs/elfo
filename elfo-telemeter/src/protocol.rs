//! Contains the protocol to interact with the telemeter.

use std::sync::Arc;

use fxhash::FxHashMap;
use metrics::Key;
use sketches_ddsketch::{Config as DDSketchConfig, DDSketch};
use tracing::warn;

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
    sketch: Arc<DDSketch>,

    // OpenMetrics requires to return cumulative counters.
    // So, we need to store them separately from the sketch.
    // These fields aren't reset on `reset()` calls.
    cumulative_sum: f64,
    cumulative_count: usize,
}

impl Default for Distribution {
    fn default() -> Self {
        Self {
            sketch: make_ddsketch(),
            cumulative_sum: 0.0,
            cumulative_count: 0,
        }
    }
}

impl Distribution {
    /// Returns the estimated value at the given quantile.
    ///
    /// Returns `None` if the quantile value is outside the range `[0.0, 1.0]`,
    /// or if the distribution is empty. Additionally, if the quantile value is
    /// invalid, a warning will be logged.
    #[inline]
    pub fn quantile(&self, q: f64) -> Option<f64> {
        self.sketch.quantile(q).unwrap_or_else(|err| {
            warn!(error = %err, "failed to calculate a quantile");
            None
        })
    }

    /// Returns the minimum value this summary has seen so far,
    /// or `None` if the distribution is empty.
    #[inline]
    pub fn min(&self) -> Option<f64> {
        self.sketch.min()
    }

    /// Returns the maximum value this summary has seen so far,
    /// or `None` if the distribution is empty.
    #[inline]
    pub fn max(&self) -> Option<f64> {
        self.sketch.max()
    }

    /// Returns the cumulative number of samples in this distribution.
    /// It isn't reset on `reset()` calls.
    #[inline]
    pub fn cumulative_count(&self) -> usize {
        self.cumulative_count + self.sketch.count()
    }

    /// Returns the cumulative sum of samples in this distribution.
    /// It isn't reset on `reset()` calls.
    #[inline]
    pub fn cumulative_sum(&self) -> f64 {
        self.cumulative_sum + self.sketch.sum().unwrap_or_default()
    }

    /// Resets the distribution. It doesn't reset cumulative values.
    pub(crate) fn reset(&mut self) {
        self.cumulative_sum += self.sketch.sum().unwrap_or_default();
        self.cumulative_count += self.sketch.count();

        self.sketch = make_ddsketch();
    }

    /// Adds samples to the distribution. Ignores all non-finite samples.
    pub(crate) fn add(&mut self, samples: &[f64]) {
        let sketch = Arc::make_mut(&mut self.sketch);

        // NOTE: We don't modify cumulative values here to reduce precision loss
        //       for long-running applications. Instead, we calculate them on demand.

        samples
            .iter()
            .filter(|v| f64::is_finite(**v))
            .for_each(|v| sketch.add(*v));
    }
}

fn make_ddsketch() -> Arc<DDSketch> {
    let max_error = 0.01; // 1% should be enough for most cases
    let max_bins = 8192; // up to 64KiB for only non-negative values
    let min_value = 1e-9; // support values down to a single nanosecond

    let config = DDSketchConfig::new(max_error, max_bins, min_value);
    Arc::new(DDSketch::new(config))
}
