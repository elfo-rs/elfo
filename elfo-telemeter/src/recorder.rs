use std::sync::Arc;

use metrics::{GaugeValue, Key, Recorder, Unit};
use metrics_util::{Handle, MetricKind};

use crate::storage::Storage;

pub(crate) struct PrometheusRecorder {
    storage: Arc<Storage>,
}

impl PrometheusRecorder {
    pub(crate) fn new(storage: Arc<Storage>) -> Self {
        Self { storage }
    }
}

impl Recorder for PrometheusRecorder {
    fn register_counter(&self, key: &Key, _unit: Option<Unit>, description: Option<&'static str>) {
        self.storage.add_description_if_missing(key, description);
        self.storage
            .registry()
            .op(MetricKind::Counter, key, |_| {}, Handle::counter);
    }

    fn register_gauge(&self, key: &Key, _unit: Option<Unit>, description: Option<&'static str>) {
        self.storage.add_description_if_missing(key, description);
        self.storage
            .registry()
            .op(MetricKind::Gauge, key, |_| {}, Handle::gauge);
    }

    fn register_histogram(
        &self,
        key: &Key,
        _unit: Option<Unit>,
        description: Option<&'static str>,
    ) {
        self.storage.add_description_if_missing(key, description);
        self.storage
            .registry()
            .op(MetricKind::Histogram, key, |_| {}, Handle::histogram);
    }

    fn increment_counter(&self, key: &Key, value: u64) {
        self.storage.registry().op(
            MetricKind::Counter,
            key,
            |h| h.increment_counter(value),
            Handle::counter,
        );
    }

    fn update_gauge(&self, key: &Key, value: GaugeValue) {
        self.storage.registry().op(
            MetricKind::Gauge,
            key,
            |h| h.update_gauge(value),
            Handle::gauge,
        );
    }

    fn record_histogram(&self, key: &Key, value: f64) {
        self.storage.registry().op(
            MetricKind::Histogram,
            key,
            |h| h.record_histogram(value),
            Handle::histogram,
        );
    }
}
