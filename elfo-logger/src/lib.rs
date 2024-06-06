//! Registers `tracing` subscriber and logs events.

#[macro_use]
extern crate elfo_utils;

use std::{env, sync::Arc, time::SystemTime};

use dashmap::DashMap;
use derive_more::Constructor;
use futures_intrusive::{buffer::GrowingHeapBuf, channel::GenericChannel};
use fxhash::FxBuildHasher;
use parking_lot::RawMutex;
use sharded_slab::Pool;
use tracing::{span::Id as SpanId, Metadata, Subscriber};
use tracing_subscriber::{prelude::*, registry::Registry, EnvFilter};

use elfo_core::{tracing::TraceId, ActorMeta, Blueprint};

use crate::{actor::Logger, filtering_layer::FilteringLayer, printing_layer::PrintingLayer};

#[cfg(feature = "docsrs")]
pub use crate::config::Config;

pub use crate::actor::ReopenLogFile;

mod actor;
mod config;
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

fn new() -> (PrintingLayer, FilteringLayer, Blueprint) {
    let shared = Shared {
        channel: GenericChannel::with_capacity(CHANNEL_CAPACITY),
        pool: Pool::default(),
        spans: DashMap::default(),
    };

    let shared = Arc::new(shared);
    let printing_layer = PrintingLayer::new(shared.clone());
    let filtering_layer = FilteringLayer::new();
    let blueprint = Logger::blueprint(shared, filtering_layer.clone());

    (printing_layer, filtering_layer, blueprint)
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
    // TODO: log instead of panicking.
    let (printer, filter, blueprint) = new();
    let registry = Registry::default();

    if env::var(EnvFilter::DEFAULT_ENV).is_ok() {
        let filter = EnvFilter::try_from_default_env().expect("invalid env");
        let subscriber = registry.with(filter).with(printer);
        install_subscriber(subscriber);
    } else {
        let subscriber = registry.with(filter).with(printer);
        install_subscriber(subscriber);
    };

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

fn install_subscriber(s: impl Subscriber + Send + Sync) {
    tracing::subscriber::set_global_default(s).expect("cannot set global subscriber");
}
