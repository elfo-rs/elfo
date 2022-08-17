use std::{
    hash::{Hash, Hasher},
    mem,
    sync::Arc,
};

use fxhash::FxHashMap;
use metrics::{GaugeValue, Key, KeyHasher};
use metrics_util::{Generational, Handle, Hashable, MetricKind, NotTracked, Registry};
use parking_lot::{RwLock, RwLockReadGuard};

use elfo_core::{scope::Scope, ActorMeta, Addr};

use crate::protocol::{Metrics, Snapshot};

pub(crate) struct Storage {
    registry: Registry<ExtKey, ExtHandle, NotTracked<ExtHandle>>,
    descriptions: RwLock<FxHashMap<String, &'static str>>,
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct ExtKey {
    group: Addr, // `Addr::NULL` if global.
    // XXX: we are forced to use hash here, because API of `Registry`
    //      isn't composable with composite keys for now.
    key_hash: u64,
}

fn make_ext_key(scope: Option<&Scope>, key: &Key, with_actor_key: bool) -> ExtKey {
    let mut key_hash = key.get_hash();

    if let Some(scope) = scope.filter(|_| with_actor_key) {
        debug_assert!(!scope.meta().key.is_empty());
        let mut hasher = KeyHasher::default();

        if let Some(telemetry_key) = scope.telemetry_key() {
            telemetry_key.hash(&mut hasher);
        } else {
            scope.meta().key.hash(&mut hasher);
        }
        key_hash ^= hasher.finish();
    }

    let group = if let Some(scope) = scope {
        debug_assert_ne!(scope.group(), Addr::NULL);
        scope.group()
    } else {
        Addr::NULL
    };

    ExtKey { group, key_hash }
}

impl Hashable for ExtKey {
    #[inline]
    fn hashable(&self) -> u64 {
        // TODO: get rid of double hashing.
        let mut hasher = KeyHasher::default();
        self.hash(&mut hasher);
        hasher.finish()
    }
}

#[derive(Clone)]
struct ExtHandle {
    meta: Option<Arc<ActorMeta>>, // `None` if global.
    telemetry_key: Option<Arc<String>>,
    with_actor_key: bool,
    key: Key,
    handle: Handle,
}

fn make_ext_handle(
    scope: Option<&Scope>,
    key: &Key,
    handle: Handle,
    with_actor_key: bool,
) -> ExtHandle {
    ExtHandle {
        meta: scope.map(|scope| scope.meta().clone()),
        telemetry_key: scope.and_then(|scope| scope.telemetry_key().clone()),
        with_actor_key,
        key: key.clone(),
        handle,
    }
}

impl Storage {
    pub(crate) fn new() -> Self {
        Self {
            registry: Registry::<ExtKey, ExtHandle, NotTracked<ExtHandle>>::untracked(),
            descriptions: Default::default(),
        }
    }

    pub(crate) fn descriptions(&self) -> RwLockReadGuard<'_, FxHashMap<String, &'static str>> {
        self.descriptions.read()
    }

    pub(crate) fn add_description_if_missing(&self, key: &Key, description: Option<&'static str>) {
        if let Some(description) = description {
            let mut descriptions = self.descriptions.write();
            if !descriptions.contains_key(key.name().to_string().as_str()) {
                descriptions.insert(key.name().to_string(), description);
            }
        }
    }

    pub(crate) fn touch_counter(&self, scope: Option<&Scope>, key: &Key, with_actor_key: bool) {
        let ext_key = make_ext_key(scope, key, with_actor_key);
        self.registry.op(
            MetricKind::Counter,
            &ext_key,
            |_| {},
            || make_ext_handle(scope, key, Handle::counter(), with_actor_key),
        );
    }

    pub(crate) fn touch_gauge(&self, scope: Option<&Scope>, key: &Key, with_actor_key: bool) {
        let ext_key = make_ext_key(scope, key, with_actor_key);
        self.registry.op(
            MetricKind::Gauge,
            &ext_key,
            |_| {},
            || make_ext_handle(scope, key, Handle::gauge(), with_actor_key),
        );
    }

    pub(crate) fn touch_histogram(&self, scope: Option<&Scope>, key: &Key, with_actor_key: bool) {
        let ext_key = make_ext_key(scope, key, with_actor_key);
        self.registry.op(
            MetricKind::Histogram,
            &ext_key,
            |_| {},
            || make_ext_handle(scope, key, Handle::histogram(), with_actor_key),
        );
    }

    pub(crate) fn increment_counter(
        &self,
        scope: Option<&Scope>,
        key: &Key,
        value: u64,
        with_actor_key: bool,
    ) {
        let ext_key = make_ext_key(scope, key, with_actor_key);
        self.registry.op(
            MetricKind::Counter,
            &ext_key,
            |h| h.handle.increment_counter(value),
            || make_ext_handle(scope, key, Handle::counter(), with_actor_key),
        );
    }

    pub(crate) fn update_gauge(
        &self,
        scope: Option<&Scope>,
        key: &Key,
        value: GaugeValue,
        with_actor_key: bool,
    ) {
        let ext_key = make_ext_key(scope, key, with_actor_key);
        self.registry.op(
            MetricKind::Gauge,
            &ext_key,
            |h| h.handle.update_gauge(value),
            || make_ext_handle(scope, key, Handle::gauge(), with_actor_key),
        );
    }

    pub(crate) fn record_histogram(
        &self,
        scope: Option<&Scope>,
        key: &Key,
        value: f64,
        with_actor_key: bool,
    ) {
        let ext_key = make_ext_key(scope, key, with_actor_key);
        self.registry.op(
            MetricKind::Histogram,
            &ext_key,
            |h| h.handle.record_histogram(value),
            || make_ext_handle(scope, key, Handle::histogram(), with_actor_key),
        );
    }

    pub(crate) fn fill_snapshot(&self, snapshot: &mut Snapshot, only_histograms: bool) -> usize {
        let mut histograms = Vec::new();
        let mut estimated_size = 0;

        self.registry.visit(|kind, (_, h)| {
            if kind == MetricKind::Histogram {
                // Defer processing to unlock the registry faster.
                histograms.push(h.get_inner().clone());
                return;
            }

            if only_histograms {
                return;
            }

            estimated_size += fill_metric(snapshot, h.get_inner());
        });

        // Process deferred histograms.
        for handle in histograms {
            estimated_size += fill_metric(snapshot, &handle);
        }

        estimated_size
    }
}

fn fill_metric(snapshot: &mut Snapshot, handle: &ExtHandle) -> usize {
    let m = get_metrics(snapshot, handle);
    let h = &handle.handle;

    let estimated_size = match h {
        Handle::Counter(_) => {
            m.counters.insert(handle.key.clone(), h.read_counter());
            8
        }
        Handle::Gauge(_) => {
            m.gauges.insert(handle.key.clone(), h.read_gauge());
            8
        }
        Handle::Histogram(_) => {
            let mut bucket_len = 0;
            let d = m.distributions.entry(handle.key.clone()).or_default();
            h.read_histogram_with_clear(|samples| {
                bucket_len += samples.len();
                d.record_samples(samples);
            });
            d.estimated_size() + 8 * bucket_len
        }
    };

    mem::size_of::<ExtKey>() + mem::size_of::<ExtHandle>() + estimated_size
}

fn get_metrics<'a>(snapshot: &'a mut Snapshot, handle: &ExtHandle) -> &'a mut Metrics {
    // If meta is known, it's a per-actor or per-group metric.
    if let Some(meta) = &handle.meta {
        if handle.with_actor_key {
            // TODO: avoid extra allocations.
            let meta = if let Some(key) = &handle.telemetry_key {
                Arc::new(ActorMeta {
                    group: meta.group.clone(),
                    key: (**key).clone(),
                })
            } else {
                meta.clone()
            };

            snapshot.per_actor.entry(meta).or_default()
        } else if snapshot.per_group.contains_key(&meta.group) {
            snapshot.per_group.get_mut(&meta.group).unwrap()
        } else {
            snapshot.per_group.entry(meta.group.clone()).or_default()
        }
    } else {
        // Otherwise, it's a global metric.
        &mut snapshot.global
    }
}
