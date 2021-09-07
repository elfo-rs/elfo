use fxhash::FxHashMap;
use metrics::Key;
use metrics_util::{Handle, MetricKind, NotTracked, Registry};
use parking_lot::{RwLock, RwLockReadGuard};

use crate::distribution::Distribution;

pub(crate) struct Storage {
    registry: Registry<Key, Handle, NotTracked<Handle>>,
    distributions: RwLock<FxHashMap<String, FxHashMap<Vec<String>, Distribution>>>,
    descriptions: RwLock<FxHashMap<String, &'static str>>,
    global_labels: RwLock<FxHashMap<String, String>>,
}

pub(crate) struct Snapshot {
    pub(crate) counters: FxHashMap<String, FxHashMap<Vec<String>, u64>>,
    pub(crate) gauges: FxHashMap<String, FxHashMap<Vec<String>, f64>>,
    pub(crate) distributions: FxHashMap<String, FxHashMap<Vec<String>, Distribution>>,
}

impl Storage {
    pub(crate) fn new() -> Self {
        Self {
            registry: Registry::<Key, Handle, NotTracked<Handle>>::untracked(),
            distributions: RwLock::new(FxHashMap::default()),
            descriptions: RwLock::new(FxHashMap::default()),
            global_labels: RwLock::new(FxHashMap::default()),
        }
    }

    pub(crate) fn configure(&self) {
        // TODO
    }

    pub(crate) fn registry(&self) -> &Registry<Key, Handle, NotTracked<Handle>> {
        &self.registry
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

    pub(crate) fn snapshot(&self) -> Snapshot {
        let metrics = self.registry().get_handles();
        let global_labels = self.global_labels.read();

        let mut counters = FxHashMap::default();
        let mut gauges = FxHashMap::default();

        for ((kind, key), (_, handle)) in metrics.into_iter() {
            match kind {
                MetricKind::Counter => {
                    let value = handle.read_counter();

                    let (name, labels) = key_to_parts(&key, &global_labels);
                    let entry = counters
                        .entry(name)
                        .or_insert_with(FxHashMap::default)
                        .entry(labels)
                        .or_insert(0);
                    *entry = value;
                }
                MetricKind::Gauge => {
                    let value = handle.read_gauge();

                    let (name, labels) = key_to_parts(&key, &global_labels);
                    let entry = gauges
                        .entry(name)
                        .or_insert_with(FxHashMap::default)
                        .entry(labels)
                        .or_insert(0.0);
                    *entry = value;
                }
                MetricKind::Histogram => {
                    let (name, labels) = key_to_parts(&key, &global_labels);

                    let mut wg = self.distributions.write();
                    let entry = wg
                        .entry(name.clone())
                        .or_insert_with(FxHashMap::default)
                        .entry(labels)
                        .or_insert_with(Distribution::new_summary);

                    handle.read_histogram_with_clear(|samples| entry.record_samples(samples));
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

fn key_to_parts(key: &Key, defaults: &FxHashMap<String, String>) -> (String, Vec<String>) {
    let name = sanitize_key_name(key.name());
    let mut values = defaults.clone();
    key.labels().into_iter().for_each(|label| {
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
