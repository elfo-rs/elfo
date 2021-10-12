#![warn(rust_2018_idioms, unreachable_pub)]

#[macro_use]
extern crate elfo_utils;

use std::{env, sync::Arc, time::SystemTime};

use dashmap::DashMap;
use derive_more::Constructor;
use futures_intrusive::{buffer::GrowingHeapBuf, channel::GenericChannel};
use fxhash::FxBuildHasher;
use parking_lot::RawMutex;
use sharded_slab::Pool;
use tracing::{span::Id as SpanId, Level, Metadata, Subscriber};
use tracing_subscriber::{prelude::*, registry::Registry, EnvFilter};

use elfo_core::{trace_id::TraceId, ActorMeta, Schema};

use crate::{actor::Logger, filtering_layer::FilteringLayer, printing_layer::PrintingLayer};

pub use crate::actor::ReopenLogFile;

mod actor;
mod config;
mod filtering_layer;
mod formatters;
mod printing_layer;
mod stats;
mod theme;

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

// TODO: revise factory (return also `FilteringLayer` somehow).
pub fn new() -> (PrintingLayer, Schema) {
    let shared = Shared {
        channel: GenericChannel::with_capacity(CHANNEL_CAPACITY),
        pool: Pool::default(),
        spans: DashMap::default(),
    };

    let shared = Arc::new(shared);
    let layer = PrintingLayer::new(shared.clone());
    let schema = Logger::new(shared);

    (layer, schema)
}

pub fn init() -> Schema {
    // TODO: log instead of panicking.
    let (printer, schema) = new();
    let registry = Registry::default();

    if env::var(EnvFilter::DEFAULT_ENV).is_ok() {
        let filter = EnvFilter::try_from_default_env().expect("invalid env");
        let subscriber = registry.with(filter).with(printer);
        install_subscriber(subscriber);
    } else {
        // TODO: get the default level via arguments.
        let filter = FilteringLayer::new(Level::INFO);
        let subscriber = registry.with(filter).with(printer);
        install_subscriber(subscriber);
    };

    schema
}

fn install_subscriber(s: impl Subscriber + Send + Sync) {
    tracing::subscriber::set_global_default(s).expect("cannot set global subscriber");
}
