use std::{
    hash::{Hash, Hasher},
    sync::Arc,
};

use fxhash::FxHashMap;
use metrics::{GaugeValue, Key, KeyHasher};
use metrics_util::{Handle, Hashable, MetricKind, NotTracked, Registry};
use parking_lot::{RwLock, RwLockReadGuard};

use elfo_core::{Addr, _priv::ObjectMeta, scope::Scope};

use crate::distribution::Distribution;

pub(crate) struct Storage {
    registry: Registry<ExtKey, ExtHandle, NotTracked<ExtHandle>>,
    distributions: RwLock<FxHashMap<String, FxHashMap<Vec<String>, Distribution>>>,
    descriptions: RwLock<FxHashMap<String, &'static str>>,
    global_labels: RwLock<FxHashMap<String, String>>,
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct ExtKey {
    group: Addr,
    // XXX: we are forced to use hash here, because API of `Registry`
    //      isn't composable with composite keys for now.
    key_hash: u64,
}

fn make_ext_key(scope: &Scope, key: &Key, with_actor_key: bool) -> ExtKey {
    let mut hash = key.get_hash();

    if with_actor_key {
        debug_assert!(scope.meta().key.is_some());
        let mut hasher = KeyHasher::default();
        scope.meta().key.hash(&mut hasher);
        hash ^= hasher.finish();
    }

    debug_assert_ne!(scope.group(), Addr::NULL);

    ExtKey {
        group: scope.group(),
        key_hash: hash,
    }
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
    meta: Arc<ObjectMeta>,
    with_actor_key: bool,
    key: Key,
    handle: Handle,
}

fn make_ext_handle(scope: &Scope, key: &Key, handle: Handle, with_actor_key: bool) -> ExtHandle {
    ExtHandle {
        meta: scope.meta().clone(),
        with_actor_key,
        key: key.clone(),
        handle,
    }
}

pub(crate) struct Snapshot {
    pub(crate) counters: FxHashMap<String, FxHashMap<Vec<String>, u64>>,
    pub(crate) gauges: FxHashMap<String, FxHashMap<Vec<String>, f64>>,
    pub(crate) distributions: FxHashMap<String, FxHashMap<Vec<String>, Distribution>>,
}

impl Storage {
    pub(crate) fn new() -> Self {
        Self {
            registry: Registry::<ExtKey, ExtHandle, NotTracked<ExtHandle>>::untracked(),
            distributions: RwLock::new(FxHashMap::default()),
            descriptions: RwLock::new(FxHashMap::default()),
            global_labels: RwLock::new(FxHashMap::default()),
        }
    }

    pub(crate) fn configure(&self, global_labels: &[(String, String)]) {
        let map = global_labels.iter().cloned().collect();
        *self.global_labels.write() = map;
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

    pub(crate) fn touch_counter(&self, scope: &Scope, key: &Key, with_actor_key: bool) {
        let ext_key = make_ext_key(scope, key, with_actor_key);
        self.registry.op(
            MetricKind::Counter,
            &ext_key,
            |_| {},
            || make_ext_handle(scope, key, Handle::counter(), with_actor_key),
        );
    }

    pub(crate) fn touch_gauge(&self, scope: &Scope, key: &Key, with_actor_key: bool) {
        let ext_key = make_ext_key(scope, key, with_actor_key);
        self.registry.op(
            MetricKind::Gauge,
            &ext_key,
            |_| {},
            || make_ext_handle(scope, key, Handle::gauge(), with_actor_key),
        );
    }

    pub(crate) fn touch_histogram(&self, scope: &Scope, key: &Key, with_actor_key: bool) {
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
        scope: &Scope,
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
        scope: &Scope,
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
        scope: &Scope,
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

    pub(crate) fn snapshot(&self) -> Snapshot {
        let metrics = self.registry.get_handles();
        let global_labels = self.global_labels.read();

        let mut counters = FxHashMap::default();
        let mut gauges = FxHashMap::default();

        for ((kind, _), (_, h)) in metrics.into_iter() {
            match kind {
                MetricKind::Counter => {
                    let value = h.handle.read_counter();

                    let (name, labels) = make_parts(&h, &global_labels);
                    let entry = counters
                        .entry(name)
                        .or_insert_with(FxHashMap::default)
                        .entry(labels)
                        .or_insert(0);
                    *entry = value;
                }
                MetricKind::Gauge => {
                    let value = h.handle.read_gauge();

                    let (name, labels) = make_parts(&h, &global_labels);
                    let entry = gauges
                        .entry(name)
                        .or_insert_with(FxHashMap::default)
                        .entry(labels)
                        .or_insert(0.0);
                    *entry = value;
                }
                MetricKind::Histogram => {
                    let (name, labels) = make_parts(&h, &global_labels);

                    let mut wg = self.distributions.write();
                    let entry = wg
                        .entry(name.clone())
                        .or_insert_with(FxHashMap::default)
                        .entry(labels)
                        .or_insert_with(Distribution::new_summary);

                    h.handle
                        .read_histogram_with_clear(|samples| entry.record_samples(samples));
                }
            }
        }

        let distributions = self.distributions.read().clone();

        Snapshot {
            counters,
            gauges,
            distributions,
        }
    }
}

fn make_parts(h: &ExtHandle, defaults: &FxHashMap<String, String>) -> (String, Vec<String>) {
    let name = sanitize_key_name(h.key.name());

    // Add global labels.
    let mut values = defaults.clone();

    // Add "actor_group" and "actor_key" labels.
    values.insert("actor_group".into(), h.meta.group.clone());
    if h.with_actor_key {
        if let Some(actor_key) = &h.meta.key {
            values.insert("actor_key".into(), actor_key.clone());
        }
    }

    // Add specific labels.
    h.key.labels().into_iter().for_each(|label| {
        values.insert(label.key().into(), label.value().into());
    });

    let labels = values
        .iter()
        .map(|(k, v)| {
            format!(
                "{}=\"{}\"",
                k,
                v.replace('\\', "\\\\")
                    .replace('"', "\\\"")
                    .replace('\n', "\\n")
            )
        })
        .collect();

    (name, labels)
}

fn sanitize_key_name(key: &str) -> String {
    // Replace anything that isn't [a-zA-Z0-9_:].
    let sanitize = |c: char| !(c.is_alphanumeric() || c == '_' || c == ':');
    key.to_string().replace(sanitize, "_")
}
