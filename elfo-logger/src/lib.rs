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
use tracing::{span::Id as SpanId, Level};
use tracing_subscriber::{prelude::*, registry::Registry, EnvFilter};

use elfo_core::{trace_id::TraceId, Schema, _priv::ObjectMeta};

use crate::{actor::Logger, layer::PrintLayer};

mod actor;
mod config;
mod formatters;
mod layer;
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
    level: Level,
    trace_id: Option<TraceId>,
    object: Option<Arc<ObjectMeta>>,
    span_id: Option<SpanId>,
    payload_id: StringId,
}

pub fn new() -> (PrintLayer, Schema) {
    let shared = Shared {
        channel: GenericChannel::with_capacity(CHANNEL_CAPACITY),
        pool: Pool::default(),
        spans: DashMap::default(),
    };

    let shared = Arc::new(shared);
    let layer = PrintLayer::new(shared.clone());
    let schema = Logger::new(shared);

    (layer, schema)
}

pub fn init() -> Schema {
    // TODO: log instead of panicking.
    let (print_layer, schema) = new();

    let filter_layer = if env::var(EnvFilter::DEFAULT_ENV).is_ok() {
        EnvFilter::try_from_default_env().expect("invalid env")
    } else {
        EnvFilter::try_new("info").unwrap()
    };

    let subscriber = Registry::default().with(filter_layer).with(print_layer);

    tracing::subscriber::set_global_default(subscriber).expect("cannot set global subscriber");
    schema
}
