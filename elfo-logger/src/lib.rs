//! Registers `tracing` subscriber and logs events. [Configuration].
//!
//! [Configuration]: config::Config

#![cfg_attr(docsrs, feature(doc_cfg))]

#[macro_use]
extern crate elfo_utils;

use std::sync::Arc;

use dashmap::DashMap;
use derive_more::Constructor;
use futures_intrusive::{buffer::GrowingHeapBuf, channel::GenericChannel};
use fxhash::FxBuildHasher;
use parking_lot::RawMutex;
use sharded_slab::Pool;
use tracing::{Metadata, span::Id as SpanId};
use tracing_subscriber::{prelude::*, registry::Registry};

use elfo_core::{ActorMeta, Blueprint, tracing::TraceId};
use elfo_utils::time::SystemTime;

use crate::actor::Logger;

pub use crate::{actor::ReopenLogFile, capture_layer::CaptureLayer, scope_filter::ScopeFilter};

pub mod config;

mod actor;
mod capture_layer;
mod formatters;
mod scope_filter;
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

/// Creates a new logger blueprint and returns [`ScopeFilter`] and
/// [`CaptureLayer`] to install on a tracing subscriber.
///
/// It's useful when you want to set a tracing subscriber on your own, for
/// example, to integrate with `console-subscriber` or to use custom layers.
///
/// # Example
/// ```
/// # use elfo_core as elfo;
/// # mod console_subscriber { pub fn spawn() -> Option<elfo_logger::CaptureLayer> { None } }
/// use tracing_subscriber::prelude::*;
///
/// // Usually, it's `elfo::batteries::logger::new`.
/// let (logger, scope_filter, capture_layer) = elfo_logger::new();
///
/// tracing_subscriber::registry()
///     .with(console_subscriber::spawn())
///     .with(capture_layer.with_filter(scope_filter))
///     .init();
///
/// let topology = elfo::Topology::empty();
/// let loggers = topology.local("system.loggers");
///
/// loggers.mount(logger);
/// ```
pub fn new() -> (Blueprint, ScopeFilter, CaptureLayer) {
    let shared = Shared {
        channel: GenericChannel::with_capacity(CHANNEL_CAPACITY),
        pool: Pool::default(),
        spans: DashMap::default(),
    };

    let shared = Arc::new(shared);
    let scope_filter = ScopeFilter::new();
    let capture_layer = CaptureLayer::new(shared.clone());
    let blueprint = Logger::blueprint(shared, scope_filter.clone());

    (blueprint, scope_filter, capture_layer)
}

/// Initializes `tracing` subscriber and returns a blueprint.
///
/// It install a subscriber with following layers:
/// * [`EnvFilter`], if `RUST_LOG` is set and the `env-filter` feature is
///   enabled (it's by default).
/// * [`ScopeFilter`] to filter events based on [elfo configuration].
/// * [`CaptureLayer`] to send events to the logger actor.
///
/// # Example
/// ```
/// # use elfo_core as elfo;
///
/// // Usually, it's `elfo::batteries::logger::init`.
/// let logger = elfo_logger::init();
///
/// let topology = elfo::Topology::empty();
/// let loggers = topology.local("system.loggers");
///
/// loggers.mount(logger);
/// ```
///
/// [`EnvFilter`]: tracing_subscriber::EnvFilter
/// [elfo configuration]: elfo_core::config::system::logging::LoggingConfig
pub fn init() -> Blueprint {
    let (blueprint, scope_filter, capture_layer) = new();

    let subscriber = Registry::default().with(capture_layer).with(scope_filter);

    #[cfg(feature = "env-filter")]
    let subscriber = {
        use tracing_subscriber::EnvFilter;

        let env_filter = std::env::var(EnvFilter::DEFAULT_ENV)
            .ok()
            .map(|_| EnvFilter::try_from_default_env().expect("invalid env"));

        subscriber.with(env_filter)
    };

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
            // Required to initialize `tracing-log` logs detection in `scope_filter`.
            log::error!("initialized tracing_log adapter");
        }
    }

    blueprint
}
