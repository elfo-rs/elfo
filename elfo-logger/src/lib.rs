//! Registers `tracing` subscriber and logs events. [Configuration].
//!
//! [Configuration]: config::Config

#[macro_use]
extern crate elfo_utils;

use std::{env, sync::Arc};

use dashmap::DashMap;
use derive_more::Constructor;
use futures_intrusive::{buffer::GrowingHeapBuf, channel::GenericChannel};
use fxhash::FxBuildHasher;
use parking_lot::RawMutex;
use sharded_slab::Pool;
use tracing::{span::Id as SpanId, Metadata};
use tracing_subscriber::{prelude::*, registry::Registry, EnvFilter};

use elfo_core::{tracing::TraceId, ActorMeta, Blueprint};
use elfo_utils::time::SystemTime;

use crate::{actor::Logger, filtering_layer::FilteringLayer, printing_layer::PrintingLayer};

pub use crate::actor::ReopenLogFile;

pub mod config;

mod actor;
mod filtering_layer;
mod formatters;
mod printing_layer;
mod stats;
mod theme;

mod line_buffer;
mod line_transaction;

const CHANNEL_CAPACITY: usize = 128 * 1024;

type StringId = usize;

struct Shared {
    channel: GenericChannel<RawMutex, PreparedEvent, GrowingHeapBuf<PreparedEvent>>,
    pool: Pool<String>,
    spans: DashMap<SpanId, SpanData, FxBuildHasher>,
}

// TODO: store once instead of here and `Registry`.
#[derive(Constructor)]
struct SpanData {
    parent_id: Option<SpanId>,
    payload_id: StringId,
}

struct PreparedEvent {
    timestamp: SystemTime,
    trace_id: Option<TraceId>,
    metadata: &'static Metadata<'static>,
    object: Option<Arc<ActorMeta>>,
    span_id: Option<SpanId>,
    payload_id: StringId,
}

// TODO: revise names of layers.
pub fn new() -> (Blueprint, FilteringLayer, PrintingLayer) {
    let shared = Shared {
        channel: GenericChannel::with_capacity(CHANNEL_CAPACITY),
        pool: Pool::default(),
        spans: DashMap::default(),
    };

    let shared = Arc::new(shared);
    let filter = FilteringLayer::new();
    let transmit = PrintingLayer::new(shared.clone());
    let blueprint = Logger::blueprint(shared, filter.clone());

    (blueprint, filter, transmit)
}

/// Initializes `tracing` subscriber and returns a blueprint.
///
/// # Example
/// ```
/// # use elfo_core as elfo;
///
/// // Usually, it's `elfo::batteries::logger::init`.
/// let logger = elfo_logger::init();
///
/// let topology = elfo::Topology::empty();
/// let loggers = topology.local("loggers");
///
/// loggers.mount(logger);
/// ```
pub fn init() -> Blueprint {
    let (blueprint, filter, transmit) = new();

    // TODO: the `env-filter` feature.
    // TODO: log instead of panicking?
    let env_filter = env::var(EnvFilter::DEFAULT_ENV)
        .ok()
        .map(|_| EnvFilter::try_from_default_env().expect("invalid env"));

    let subscriber = Registry::default()
        .with(transmit)
        .with(filter)
        .with(env_filter);

    tracing::subscriber::set_global_default(subscriber).expect("cannot set global subscriber");

    // TODO: remove test log at all?
    #[cfg(feature = "tracing-log")]
    {
        if let Err(e) = tracing_log::LogTracer::init() {
            tracing::error!(
                error = &*e.to_string(),
                "can't integrate with log adapter, logs produced via `log` crate might be not available",
            );
        } else {
            // Required to initialize `tracing-log` logs detection in `filtering_layer`.
            log::error!("initialized tracing_log adapter");
        }
    }

    blueprint
}
