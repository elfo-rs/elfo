use std::sync::Arc;

use metrics::{GaugeValue, Key, Unit};

use elfo_core::{scope, telemetry};

use crate::{
    metrics::{Counter, Gauge, Histogram},
    storage::{ActorScope, GlobalScope, GroupScope, Storable, Storage},
};

pub(crate) struct Recorder {
    storage: Arc<Storage>,
}

impl Recorder {
    pub(crate) fn new(storage: Arc<Storage>) -> Self {
        Self { storage }
    }

    fn record<M>(&self, key: &Key, value: M::Value)
    where
        M: Storable,
        M::Value: Clone,
    {
        scope::try_with(|scope| {
            let perm = scope.permissions();

            if perm.is_telemetry_per_actor_group_enabled() {
                self.storage
                    .upsert::<GroupScope, M>(scope, key, value.clone())
            }

            if perm.is_telemetry_per_actor_key_enabled() {
                // TODO: get rid of this check.
                if scope.telemetry_meta().key.is_empty() {
                    return;
                }

                self.storage
                    .upsert::<ActorScope, M>(scope, key, value.clone())
            }
        })
        .unwrap_or_else(|| self.storage.upsert::<GlobalScope, M>(&(), key, value))
    }
}

impl metrics::Recorder for Recorder {
    fn register_counter(&self, key: &Key, unit: Option<Unit>, description: Option<&'static str>) {
        self.storage.describe(key, unit, description);
    }

    fn register_gauge(&self, key: &Key, unit: Option<Unit>, description: Option<&'static str>) {
        self.storage.describe(key, unit, description);
    }

    fn register_histogram(&self, key: &Key, unit: Option<Unit>, description: Option<&'static str>) {
        self.storage.describe(key, unit, description);
    }

    fn increment_counter(&self, key: &Key, value: u64) {
        self.record::<Counter>(key, value)
    }

    fn update_gauge(&self, key: &Key, value: GaugeValue) {
        self.record::<Gauge>(key, value)
    }

    fn record_histogram(&self, key: &Key, value: f64) {
        self.record::<Histogram>(key, value)
    }
}

impl telemetry::Recorder for Storage {
    fn enter(&self) {
        self.enter();
    }

    fn exit(&self) {
        self.exit();
    }
}
