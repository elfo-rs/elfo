#![allow(private_interfaces)]

use std::{hash::Hash, mem, sync::Arc};

use fxhash::FxHashMap;
use metrics::{Key, Unit};
use parking_lot::{Mutex, MutexGuard};
use thread_local::ThreadLocal;

use elfo_core::{coop, scope::Scope, ActorMeta, Addr};

use crate::{
    metrics::{Counter, Gauge, GaugeOrigin, Histogram, MetricKind},
    protocol::{Description, Metrics, Snapshot},
    stats::{ShardStats, StorageStats},
};

// === Scopes ===

// TODO: use real `Key` (with pointer comparison).
type KeyHash = u64;

pub(crate) trait ScopeKind: Sized {
    type Scope;
    type Key: Copy + Hash + Eq;
    type Meta: Clone;

    fn get_meta(scope: &Self::Scope) -> &Self::Meta;
    fn make_key(scope: &Self::Scope, key: &Key) -> Self::Key;
    fn registries(shard: &Shard) -> &Registries<Self>;
    fn gauge_shared(storage: &Storage) -> &Mutex<GaugeOrigins<Self>>;
    fn snapshot<'s>(snapshot: &'s mut Snapshot, meta: &Self::Meta) -> &'s mut Metrics;
}

pub(crate) struct GlobalScope;

impl ScopeKind for GlobalScope {
    type Key = KeyHash;
    type Meta = ();
    type Scope = ();

    fn get_meta(_scope: &Self::Scope) -> &Self::Meta {
        &()
    }

    fn make_key(_scope: &Self::Scope, key: &Key) -> Self::Key {
        key.get_hash()
    }

    fn registries(shard: &Shard) -> &Registries<Self> {
        &shard.global
    }

    fn gauge_shared(storage: &Storage) -> &Mutex<GaugeOrigins<Self>> {
        &storage.gauge_shared.global
    }

    fn snapshot<'s>(snapshot: &'s mut Snapshot, _meta: &Self::Meta) -> &'s mut Metrics {
        &mut snapshot.global
    }
}

pub(crate) struct GroupScope;

impl ScopeKind for GroupScope {
    type Key = (Addr, KeyHash);
    // TODO: replace with EBR?
    type Meta = Arc<ActorMeta>;
    type Scope = Scope;

    fn get_meta(scope: &Self::Scope) -> &Self::Meta {
        scope.telemetry_meta()
    }

    fn make_key(scope: &Self::Scope, key: &Key) -> Self::Key {
        debug_assert_ne!(scope.group(), Addr::NULL);
        (scope.group(), key.get_hash())
    }

    fn registries(shard: &Shard) -> &Registries<Self> {
        &shard.groupwise
    }

    fn gauge_shared(storage: &Storage) -> &Mutex<GaugeOrigins<Self>> {
        &storage.gauge_shared.groupwise
    }

    fn snapshot<'s>(snapshot: &'s mut Snapshot, meta: &Self::Meta) -> &'s mut Metrics {
        snapshot.groupwise.entry(meta.group.clone()).or_default()
    }
}

pub(crate) struct ActorScope;

impl ScopeKind for ActorScope {
    type Key = (/* group */ Addr, KeyHash);
    // TODO: replace with EBR?
    type Meta = Arc<ActorMeta>;
    type Scope = Scope;

    fn get_meta(scope: &Self::Scope) -> &Self::Meta {
        scope.telemetry_meta()
    }

    fn make_key(scope: &Self::Scope, key: &Key) -> Self::Key {
        debug_assert_ne!(scope.group(), Addr::NULL);

        let telemetry_key = &scope.telemetry_meta().key;
        debug_assert!(!telemetry_key.is_empty());

        // TODO: cache a hash of the telemetry key.
        let key_hash = fxhash::hash64(&(telemetry_key, key.get_hash()));
        (scope.group(), key_hash)
    }

    fn registries(shard: &Shard) -> &Registries<Self> {
        &shard.actorwise
    }

    fn gauge_shared(storage: &Storage) -> &Mutex<GaugeOrigins<Self>> {
        &storage.gauge_shared.actorwise
    }

    fn snapshot<'s>(snapshot: &'s mut Snapshot, meta: &Self::Meta) -> &'s mut Metrics {
        snapshot.actorwise.entry(meta.clone()).or_default()
    }
}

// === Storage ===

/// The storage for metrics.
///
/// The main idea here is to have a separate shard for each thread and merge
/// them periodically by the telemeter actor. It *dramatically* reduces
/// contention, especially if multiple actors use the same telemetry key,
/// what's common for per-group telemetry or per-actor grouped telemetry.
pub(crate) struct Storage {
    shards: ThreadLocal<Shard>,
    // Shared gauge origins between shards. See `Gauge` for more details.
    gauge_shared: GaugeShared,
    descriptions: Mutex<FxHashMap<String, Description>>,
}

#[derive(Default)]
struct Shard {
    global: Registries<GlobalScope>,
    groupwise: Registries<GroupScope>,
    actorwise: Registries<ActorScope>,
}

// Most of the time, the mutexes are uncontended, because they are accessed only
// by one thread. Periodically, they are accessed by the telemeter actor for the
// short period of time in order to replace these registries with empty ones.
struct Registries<S: ScopeKind> {
    counters: Mutex<Registry<S, Counter>>,
    gauges: Mutex<Registry<S, Gauge>>,
    histograms: Mutex<Registry<S, Histogram>>,
}

impl<S: ScopeKind> Default for Registries<S> {
    fn default() -> Self {
        Self {
            counters: Default::default(),
            gauges: Default::default(),
            histograms: Default::default(),
        }
    }
}

type Registry<S, M> = FxHashMap<<S as ScopeKind>::Key, RegEntry<S, M>>;

struct RegEntry<S: ScopeKind, M> {
    key: Key,
    data: M,
    meta: S::Meta,
}

impl<S: ScopeKind, M: MetricKind> RegEntry<S, M> {
    #[cold]
    fn new(scope: &S::Scope, key: &Key, shared: M::Shared) -> Self {
        Self {
            key: key.clone(),
            data: M::new(shared),
            meta: S::get_meta(scope).clone(),
        }
    }
}

#[derive(Default)]
struct GaugeShared {
    global: Mutex<GaugeOrigins<GlobalScope>>,
    groupwise: Mutex<GaugeOrigins<GroupScope>>,
    actorwise: Mutex<GaugeOrigins<ActorScope>>,
}

type GaugeOrigins<S> = FxHashMap<<S as ScopeKind>::Key, Arc<GaugeOrigin>>;

impl Default for Storage {
    fn default() -> Self {
        Self {
            shards: ThreadLocal::new(),
            gauge_shared: Default::default(),
            descriptions: Default::default(),
        }
    }
}

impl Storage {
    pub(crate) fn descriptions(&self) -> MutexGuard<'_, FxHashMap<String, Description>> {
        self.descriptions.lock()
    }

    pub(crate) fn describe(&self, key: &Key, unit: Option<Unit>, details: Option<&'static str>) {
        if unit.is_none() && details.is_none() {
            return;
        }

        let mut descriptions = self.descriptions.lock();
        descriptions.insert(key.name().to_string(), Description { details, unit });
    }

    pub(crate) fn upsert<S, M>(&self, scope: &S::Scope, key: &Key, value: M::Value)
    where
        S: ScopeKind,
        M: Storable,
    {
        let shard = self.shards.get_or_default();
        let registries = S::registries(shard);
        let reg_key = S::make_key(scope, key);
        let mut registry = M::registry(registries).lock();

        let entry = registry.entry(reg_key).or_insert_with(|| {
            let shared = M::shared::<S>(self, reg_key);
            RegEntry::<S, M>::new(scope, key, shared)
        });

        entry.data.update(value);
    }

    pub(crate) async fn merge(&self, snapshot: &mut Snapshot, only_compact: bool) {
        let mut storage_stats = StorageStats::new::<Self>();

        if !only_compact {
            storage_stats.add_descriptions(&*self.descriptions.lock());
        }

        for shard in self.shards.iter() {
            let mut stats = ShardStats::new::<Shard>();

            self.merge_registries::<GlobalScope>(shard, snapshot, only_compact, &mut stats)
                .await;
            self.merge_registries::<GroupScope>(shard, snapshot, only_compact, &mut stats)
                .await;
            self.merge_registries::<ActorScope>(shard, snapshot, only_compact, &mut stats)
                .await;

            storage_stats.add_shard(&stats);
        }

        if !only_compact {
            storage_stats.emit();
        }
    }

    async fn merge_registries<S: ScopeKind>(
        &self,
        shard: &Shard,
        snapshot: &mut Snapshot,
        only_compact: bool,
        stats: &mut ShardStats,
    ) {
        let registries = S::registries(shard);

        if !only_compact {
            self.merge_registry::<S, Counter>(registries, snapshot, stats)
                .await;
            self.merge_registry::<S, Gauge>(registries, snapshot, stats)
                .await;
        }
        self.merge_registry::<S, Histogram>(registries, snapshot, stats)
            .await;
    }

    async fn merge_registry<S: ScopeKind, M: Storable>(
        &self,
        registries: &Registries<S>,
        snapshot: &mut Snapshot,
        stats: &mut ShardStats,
    ) {
        let registry = M::registry(registries);

        let registry = {
            let empty = Registry::with_hasher(<_>::default());
            let mut registry = registry.lock();
            mem::replace(&mut *registry, empty)
        };

        stats.add_registry(&registry);

        for (_, entry) in registry.into_iter() {
            let metrics = S::snapshot(snapshot, &entry.meta);
            let out = M::snapshot(metrics, &entry.key);
            let additional_size = entry.data.merge(out);
            stats.add_additional_size(additional_size);
        }

        // The merge process can be quite long, so we should be preemptive.
        coop::consume_budget().await;
    }
}

// === Storable ===

pub(crate) trait Storable: MetricKind {
    fn registry<S: ScopeKind>(registries: &Registries<S>) -> &Mutex<Registry<S, Self>>;
    fn shared<S: ScopeKind>(storage: &Storage, key: S::Key) -> Self::Shared;
    fn snapshot<'s>(metrics: &'s mut Metrics, key: &Key) -> &'s mut Self::Output;
}

impl Storable for Counter {
    fn registry<S: ScopeKind>(registries: &Registries<S>) -> &Mutex<Registry<S, Self>> {
        &registries.counters
    }

    fn shared<S: ScopeKind>(_: &Storage, _: S::Key) -> Self::Shared {}

    fn snapshot<'s>(metrics: &'s mut Metrics, key: &Key) -> &'s mut Self::Output {
        // TODO: hashbrown `entry_ref` (extra crate) or `contains_key` (double lookup).
        metrics.counters.entry(key.clone()).or_default()
    }
}

impl Storable for Gauge {
    fn registry<S: ScopeKind>(registries: &Registries<S>) -> &Mutex<Registry<S, Self>> {
        &registries.gauges
    }

    fn shared<S: ScopeKind>(storage: &Storage, key: S::Key) -> Self::Shared {
        let mut shared = S::gauge_shared(storage).lock();
        shared.entry(key).or_default().clone()
    }

    fn snapshot<'s>(metrics: &'s mut Metrics, key: &Key) -> &'s mut Self::Output {
        // TODO: hashbrown `entry_ref` (extra crate) or `contains_key` (double lookup).
        metrics.gauges.entry(key.clone()).or_default()
    }
}

impl Storable for Histogram {
    fn registry<S: ScopeKind>(registries: &Registries<S>) -> &Mutex<Registry<S, Self>> {
        &registries.histograms
    }

    fn shared<S: ScopeKind>(_: &Storage, _: S::Key) -> Self::Shared {}

    fn snapshot<'s>(metrics: &'s mut Metrics, key: &Key) -> &'s mut Self::Output {
        // TODO: hashbrown `entry_ref` (extra crate) or `contains_key` (double lookup).
        metrics.histograms.entry(key.clone()).or_default()
    }
}
