//! Interaction with the `metrics` crate.
//! Records metrics in the Prometheus exposition format.
//!
//! A lot of code here is highly inspired by `metrics-exporter-prometheus`, and
//! even copy-pasted from it with removing some useful features. Firstly, push
//! gateways aren't supported. Secondly, histogram overrides don't work, only
//! summaries.
//!
//! All metrics include information about the actor, where they were produced.
//! Such information is added as labels. By default, only the `actor_group`
//! label is added, but it's possible to provide `actor_key` on a group basis.
//! It's useful, if a group has few actors inside.
//!
//! I'm going to extend the original crate to reuse code.

#![warn(rust_2018_idioms, unreachable_pub, missing_docs)]

use std::sync::Arc;

use tracing::error;

use elfo_core::Schema;

use self::{recorder::Recorder, storage::Storage};

pub mod protocol;

mod actor;
mod config;
mod recorder;
mod render;
mod storage;

#[cfg(feature = "unstable")]
mod allocator;

#[cfg(feature = "unstable")]
pub use allocator::AllocatorStats;

/// Installs a global metric recorder and returns a group to handle metrics.
pub fn new() -> Schema {
    let storage = Arc::new(Storage::new());
    let recorder = Recorder::new(storage.clone());
    let schema = actor::new(storage);

    if let Err(err) = metrics::set_boxed_recorder(Box::new(recorder)) {
        error!(error = %err, "failed to set a metric recorder");
    }

    schema
}
