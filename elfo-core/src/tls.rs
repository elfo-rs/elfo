use std::{cell::Cell, future::Future, sync::Arc};

use crate::{object::ObjectMeta, trace_id::TraceId};

tokio::task_local! {
    static META: Arc<ObjectMeta>;
    static TRACE_ID: Cell<TraceId>;
}

/// Returns the current trace id.
///
/// # Panics
/// This function will panic if called ouside actors.
pub fn trace_id() -> TraceId {
    TRACE_ID.with(Cell::get)
}

/// Returns the current trace id if inside the actor system.
pub fn try_trace_id() -> Option<TraceId> {
    TRACE_ID.try_with(Cell::get).ok()
}

/// Replaces the current trace id with the provided one.
///
/// # Panics
/// This function will panic if called ouside actors.
pub fn set_trace_id(trace_id: TraceId) {
    TRACE_ID.with(|stored| stored.set(trace_id));
}

/// Returns the current object's meta.
///
/// # Panics
/// This function will panic if called ouside actors.
pub fn meta() -> Arc<ObjectMeta> {
    META.with(Arc::clone)
}

/// Returns the current object's meta if inside the actor system.
pub fn try_meta() -> Option<Arc<ObjectMeta>> {
    META.try_with(Arc::clone).ok()
}

pub(crate) async fn scope<F: Future>(meta: Arc<ObjectMeta>, trace_id: TraceId, f: F) -> F::Output {
    META.scope(meta, TRACE_ID.scope(Cell::new(trace_id), f))
        .await
}

pub(crate) fn sync_scope<R>(meta: Arc<ObjectMeta>, trace_id: TraceId, f: impl FnOnce() -> R) -> R {
    META.sync_scope(meta, || TRACE_ID.sync_scope(Cell::new(trace_id), f))
}
