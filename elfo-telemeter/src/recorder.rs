use std::sync::Arc;

use metrics::{GaugeValue, Key, Recorder, Unit};

use elfo_core::scope::{self, Scope};

use crate::storage::Storage;

pub(crate) struct PrometheusRecorder {
    storage: Arc<Storage>,
}

impl PrometheusRecorder {
    pub(crate) fn new(storage: Arc<Storage>) -> Self {
        Self { storage }
    }

    fn with_params(&self, f: impl Fn(&Storage, &Scope, bool)) {
        scope::try_with(|scope| {
            let perm = scope.permissions();
            if perm.is_telemetry_per_actor_group_enabled() {
                f(&self.storage, scope, false)
            }
            if perm.is_telemetry_per_actor_key_enabled() && !scope.meta().key.is_empty() {
                f(&self.storage, scope, true)
            }
        });
    }
}

impl Recorder for PrometheusRecorder {
    fn register_counter(&self, key: &Key, _unit: Option<Unit>, description: Option<&'static str>) {
        self.storage.add_description_if_missing(key, description);
        self.with_params(|storage, scope, with_actor_key| {
            storage.touch_counter(scope, key, with_actor_key)
        });
    }

    fn register_gauge(&self, key: &Key, _unit: Option<Unit>, description: Option<&'static str>) {
        self.storage.add_description_if_missing(key, description);
        self.with_params(|storage, scope, with_actor_key| {
            storage.touch_gauge(scope, key, with_actor_key)
        });
    }

    fn register_histogram(
        &self,
        key: &Key,
        _unit: Option<Unit>,
        description: Option<&'static str>,
    ) {
        self.storage.add_description_if_missing(key, description);
        self.with_params(|storage, scope, with_actor_key| {
            storage.touch_histogram(scope, key, with_actor_key)
        });
    }

    fn increment_counter(&self, key: &Key, value: u64) {
        self.with_params(|storage, scope, with_actor_key| {
            storage.increment_counter(scope, key, value, with_actor_key)
        });
    }

    fn update_gauge(&self, key: &Key, value: GaugeValue) {
        self.with_params(|storage, scope, with_actor_key| {
            storage.update_gauge(scope, key, value.clone(), with_actor_key)
        });
    }

    fn record_histogram(&self, key: &Key, value: f64) {
        self.with_params(|storage, scope, with_actor_key| {
            storage.record_histogram(scope, key, value, with_actor_key)
        });
    }
}
