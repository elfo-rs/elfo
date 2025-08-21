//! Interaction with the `metrics` crate. [Configuration].
//!
//! Records metrics in the OpenMetrics exposition format.
//!
//! Note that push gateways aren't supported, histogram buckets overrides
//! don't work, only summaries.
//!
//! All metrics include information about the actor, where they were produced.
//! Such information is added as labels. By default, only the `actor_group`
//! label is added, but it's possible to provide `actor_key` on a group basis.
//! It's useful, if a group has few actors inside.
//!
//! [Configuration]: config::Config

use std::sync::Arc;

use tracing::error;

use elfo_core::Blueprint;

use self::{recorder::Recorder, storage::Storage};

pub mod config;
pub mod protocol;

mod actor;
mod hyper;
mod metrics;
mod recorder;
mod render;
mod stats;
mod storage;

#[cfg(feature = "unstable")]
mod allocator;

#[cfg(feature = "unstable")]
pub use allocator::AllocatorStats;

/// Installs a global metric recorder and returns a group to handle metrics.
///
/// # Example
/// ```
/// # use elfo_core as elfo;
///
/// let topology = elfo::Topology::empty();
/// let telemeters = topology.local("telemeters");
///
/// // Usually, it's `elfo::batteries::telemeter::init`.
/// telemeters.mount(elfo_telemeter::init());
/// ```
pub fn init() -> Blueprint {
    let storage = Arc::new(Storage::default());
    let recorder = Recorder::new(storage.clone());
    let recorder_2 = storage.clone();
    let blueprint = actor::new(storage);

    match ::metrics::set_boxed_recorder(Box::new(recorder)) {
        Ok(_) => stats::register(),
        Err(err) => error!(error = %err, "failed to set a metric recorder"),
    }

    elfo_core::telemetry::set_recorder(recorder_2);

    blueprint
}
