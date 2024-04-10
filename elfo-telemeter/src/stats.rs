use std::mem;

use fxhash::FxHashMap;
use metrics::{gauge, register_gauge, Unit};

pub(crate) fn register() {
    register_gauge!(
        "elfo_metrics_usage_bytes",
        Unit::Bytes,
        "Total size occupied by metrics"
    );
    register_gauge!(
        "elfo_metrics_storage_shards",
        Unit::Count,
        "The number of storage shards"
    );
}

// === Storage ===

// The total size estimator.
pub(crate) struct StorageStats {
    shards_total: u32,
    shards_active: u32,
    total_size: usize,
}

impl StorageStats {
    pub(crate) fn new<T>() -> Self {
        Self {
            shards_total: 0,
            shards_active: 0,
            total_size: mem::size_of::<T>(),
        }
    }

    pub(crate) fn add_shard(&mut self, stats: &ShardStats) {
        self.shards_total += 1;
        self.shards_active += stats.has_metrics as u32;
        self.total_size += stats.size;
    }

    pub(crate) fn add_descriptions<K, V>(&mut self, registry: &FxHashMap<K, V>) {
        self.total_size += estimate_hashbrown_size::<(K, V)>(registry.capacity());
    }

    pub(crate) fn emit(&self) {
        let shards_inactive = self.shards_total - self.shards_active;
        gauge!("elfo_metrics_usage_bytes", self.total_size as f64, "object" => "Storage");
        gauge!("elfo_metrics_storage_shards", self.shards_active as f64, "status" => "Active");
        gauge!("elfo_metrics_storage_shards", shards_inactive as f64, "status" => "Inactive");
    }
}

pub(crate) struct ShardStats {
    has_metrics: bool,
    size: usize,
}

impl ShardStats {
    pub(crate) fn new<T>() -> Self {
        Self {
            has_metrics: false,
            size: mem::size_of::<T>(),
        }
    }

    pub(crate) fn add_registry<K, V>(&mut self, registry: &FxHashMap<K, V>) {
        self.has_metrics |= !registry.is_empty();
        self.size += estimate_hashbrown_size::<(K, V)>(registry.capacity());
    }

    pub(crate) fn add_additional_size(&mut self, size: usize) {
        self.size += size;
    }
}

// === Snapshot ===

pub(crate) struct SnapshotStats {
    total_size: usize,
}

impl SnapshotStats {
    pub(crate) fn new<T>() -> Self {
        Self {
            total_size: mem::size_of::<T>(),
        }
    }

    pub(crate) fn add_registry<K, V>(&mut self, registry: &FxHashMap<K, V>) {
        self.total_size += estimate_hashbrown_size::<(K, V)>(registry.capacity());
    }

    pub(crate) fn add_additional_size(&mut self, size: usize) {
        self.total_size += size;
    }

    pub(crate) fn emit(&self) {
        gauge!("elfo_metrics_usage_bytes", self.total_size as f64, "object" => "Snapshot");
    }
}

// === Helpers ===

// From the `datasize` crate.
fn estimate_hashbrown_size<T>(capacity: usize) -> usize {
    // https://github.com/rust-lang/hashbrown/blob/v0.12.3/src/raw/mod.rs#L185
    let buckets = if capacity < 8 {
        if capacity < 4 {
            4
        } else {
            8
        }
    } else {
        (capacity * 8 / 7).next_power_of_two()
    };
    // https://github.com/rust-lang/hashbrown/blob/v0.12.3/src/raw/mod.rs#L242
    let size = mem::size_of::<T>();
    // `Group` is u32, u64, or __m128i depending on the CPU architecture.
    // Return a lower bound, ignoring its constant contributions
    // (through ctrl_align and Group::WIDTH, at most 31 bytes).
    let ctrl_offset = size * buckets;
    // Add one byte of "control" metadata per bucket
    ctrl_offset + buckets
}
