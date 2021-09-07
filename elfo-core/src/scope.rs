use std::{cell::Cell, future::Future, sync::Arc};

use crate::{
    addr::Addr,
    object::ObjectMeta,
    permissions::{AtomicPermissions, Permissions},
    trace_id::{self, TraceId},
};

tokio::task_local! {
    static SCOPE: Scope;
}

#[derive(Clone)]
pub struct Scope {
    addr: Addr,
    meta: Arc<ObjectMeta>,
    trace_id: Cell<TraceId>,
    permissions: Arc<AtomicPermissions>,
}

assert_impl_all!(Scope: Send);
assert_not_impl_all!(Scope: Sync);

impl Scope {
    #[doc(hidden)]
    pub fn new(addr: Addr, meta: Arc<ObjectMeta>, perm: Arc<AtomicPermissions>) -> Self {
        Self::with_trace_id(trace_id::generate(), addr, meta, perm)
    }

    #[doc(hidden)]
    pub fn with_trace_id(
        trace_id: TraceId,
        addr: Addr,
        meta: Arc<ObjectMeta>,
        permissions: Arc<AtomicPermissions>,
    ) -> Self {
        Self {
            addr,
            meta,
            trace_id: Cell::new(trace_id),
            permissions,
        }
    }

    #[inline]
    pub fn addr(&self) -> Addr {
        self.addr
    }

    /// Returns the current object's meta.
    #[inline]
    pub fn meta(&self) -> &Arc<ObjectMeta> {
        &self.meta
    }

    /// Returns the current trace id.
    #[inline]
    pub fn trace_id(&self) -> TraceId {
        self.trace_id.get()
    }

    /// Replaces the current trace id with the provided one.
    #[inline]
    pub fn set_trace_id(&self, trace_id: TraceId) {
        self.trace_id.set(trace_id);
    }

    #[inline]
    pub fn permissions(&self) -> Permissions {
        self.permissions.load()
    }

    /// Wraps the provided future with the current scope.
    pub async fn within<F: Future>(self, f: F) -> F::Output {
        SCOPE.scope(self, f).await
    }

    /// Runs the provided function with the current scope.
    pub fn sync_within<R>(self, f: impl FnOnce() -> R) -> R {
        SCOPE.sync_scope(self, f)
    }
}

/// Exposes the current scope in order to send to other tasks.
///
/// # Panics
/// This function will panic if called outside actors.
pub fn expose() -> Scope {
    SCOPE.with(Clone::clone)
}

/// Exposes the current scope if inside the actor system.
pub fn try_expose() -> Option<Scope> {
    SCOPE.try_with(Clone::clone).ok()
}

/// Accesses the current scope and runs the provided closure.
///
/// # Panics
/// This function will panic if called ouside the actor system.
#[inline]
pub fn with<R>(f: impl FnOnce(&Scope) -> R) -> R {
    try_with(f).expect("cannot access a scope outside the actor system")
}

/// Accesses the current scope and runs the provided closure.
///
/// Returns `None` if called outside the actor system.
/// For a panicking variant, see `with`.
#[inline]
pub fn try_with<R>(f: impl FnOnce(&Scope) -> R) -> Option<R> {
    SCOPE.try_with(|scope| f(scope)).ok()
}

/// Returns the current trace id.
///
/// # Panics
/// This function will panic if called ouside the actor system.
#[inline]
pub fn trace_id() -> TraceId {
    with(Scope::trace_id)
}

/// Returns the current trace id if inside the actor system.
#[inline]
pub fn try_trace_id() -> Option<TraceId> {
    try_with(Scope::trace_id)
}

/// Replaces the current trace id with the provided one.
///
/// # Panics
/// This function will panic if called ouside the actor system.
#[inline]
pub fn set_trace_id(trace_id: TraceId) {
    with(|scope| scope.set_trace_id(trace_id));
}

/// Returns the current object's meta.
///
/// # Panics
/// This function will panic if called ouside the actor system.
#[inline]
pub fn meta() -> Arc<ObjectMeta> {
    with(|scope| scope.meta().clone())
}

/// Returns the current object's meta if inside the actor system.
#[inline]
pub fn try_meta() -> Option<Arc<ObjectMeta>> {
    try_with(|scope| scope.meta().clone())
}
