#![allow(clippy::declare_interior_mutable_const)] // see tokio#4872

use std::{
    cell::Cell,
    future::Future,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};

use crate::{
    actor::ActorMeta,
    addr::Addr,
    config::SystemConfig,
    dumping::DumpingControl,
    logging::_priv::LoggingControl,
    permissions::{AtomicPermissions, Permissions},
    telemetry::TelemetryConfig,
    tracing::TraceId,
};

tokio::task_local! {
    static SCOPE: Scope;
}

#[derive(Clone)]
pub struct Scope {
    trace_id: Cell<TraceId>,
    actor: Arc<ScopeActorShared>,
    group: Arc<ScopeGroupShared>,
}

assert_impl_all!(Scope: Send);
assert_not_impl_all!(Scope: Sync);

impl Scope {
    /// Private API for now.
    #[doc(hidden)]
    pub fn test(actor: Addr, meta: Arc<ActorMeta>) -> Self {
        Self::new(
            TraceId::generate(),
            actor,
            meta,
            Arc::new(ScopeGroupShared::new(Addr::NULL)),
        )
    }

    pub(crate) fn new(
        trace_id: TraceId,
        addr: Addr,
        meta: Arc<ActorMeta>,
        group: Arc<ScopeGroupShared>,
    ) -> Self {
        Self {
            trace_id: Cell::new(trace_id),
            actor: Arc::new(ScopeActorShared::new(addr, meta)),
            group,
        }
    }

    pub(crate) fn with_telemetry(mut self, config: &TelemetryConfig) -> Self {
        self.actor = Arc::new(self.actor.with_telemetry(config));
        self
    }

    #[inline]
    #[deprecated(note = "use `actor()` instead")]
    pub fn addr(&self) -> Addr {
        self.actor()
    }

    #[inline]
    pub fn actor(&self) -> Addr {
        self.actor.addr
    }

    #[inline]
    pub fn group(&self) -> Addr {
        self.group.addr
    }

    /// Returns the current object's meta.
    #[inline]
    pub fn meta(&self) -> &Arc<ActorMeta> {
        &self.actor.meta
    }

    /// Private API for now.
    #[inline]
    #[stability::unstable]
    #[doc(hidden)]
    pub fn telemetry_meta(&self) -> &Arc<ActorMeta> {
        &self.actor.telemetry_meta
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

    /// Returns the current permissions (for logging, telemetry and so on).
    #[inline]
    pub fn permissions(&self) -> Permissions {
        self.group.permissions.load()
    }

    /// Private API for now.
    #[inline]
    #[stability::unstable]
    #[doc(hidden)]
    pub fn logging(&self) -> &LoggingControl {
        &self.group.logging
    }

    /// Private API for now.
    #[inline]
    #[stability::unstable]
    #[doc(hidden)]
    pub fn dumping(&self) -> &DumpingControl {
        &self.group.dumping
    }

    #[doc(hidden)]
    #[stability::unstable]
    pub fn increment_allocated_bytes(&self, by: usize) {
        self.actor.allocated_bytes.fetch_add(by, Ordering::Relaxed);
    }

    #[doc(hidden)]
    #[stability::unstable]
    pub fn increment_deallocated_bytes(&self, by: usize) {
        self.actor
            .deallocated_bytes
            .fetch_add(by, Ordering::Relaxed);
    }

    pub(crate) fn take_allocated_bytes(&self) -> usize {
        self.actor.allocated_bytes.swap(0, Ordering::Relaxed)
    }

    pub(crate) fn take_deallocated_bytes(&self) -> usize {
        self.actor.deallocated_bytes.swap(0, Ordering::Relaxed)
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

struct ScopeActorShared {
    addr: Addr,
    meta: Arc<ActorMeta>,
    telemetry_meta: Arc<ActorMeta>,
    allocated_bytes: AtomicUsize,
    deallocated_bytes: AtomicUsize,
}

impl ScopeActorShared {
    fn new(addr: Addr, meta: Arc<ActorMeta>) -> Self {
        Self {
            addr,
            meta: meta.clone(),
            telemetry_meta: meta,
            allocated_bytes: AtomicUsize::new(0),
            deallocated_bytes: AtomicUsize::new(0),
        }
    }

    fn with_telemetry(&self, config: &TelemetryConfig) -> Self {
        Self {
            addr: self.addr,
            meta: self.meta.clone(),
            telemetry_meta: config
                .per_actor_key
                .key(&self.meta.key)
                .map(|key| {
                    Arc::new(ActorMeta {
                        group: self.meta.group.clone(),
                        key,
                    })
                })
                .unwrap_or_else(|| self.meta.clone()),
            allocated_bytes: AtomicUsize::new(0),
            deallocated_bytes: AtomicUsize::new(0),
        }
    }
}

assert_impl_all!(ScopeGroupShared: Send, Sync);

pub(crate) struct ScopeGroupShared {
    addr: Addr,
    permissions: AtomicPermissions,
    logging: LoggingControl,
    dumping: DumpingControl,
}

assert_impl_all!(ScopeGroupShared: Send, Sync);

impl ScopeGroupShared {
    pub(crate) fn new(addr: Addr) -> Self {
        Self {
            addr,
            permissions: Default::default(), // everything is disabled
            logging: Default::default(),
            dumping: Default::default(),
        }
    }

    pub(crate) fn configure(&self, config: &SystemConfig) {
        // Update the logging subsystem.
        self.logging.configure(&config.logging);
        let max_level = self.logging.max_level_hint().into_level();

        // Update the dumping subsystem.
        self.dumping.configure(&config.dumping);

        // Update permissions.
        let mut perm = self.permissions.load();
        perm.set_logging_enabled(max_level);
        perm.set_dumping_enabled(!config.dumping.disabled);
        perm.set_telemetry_per_actor_group_enabled(config.telemetry.per_actor_group);
        perm.set_telemetry_per_actor_key_enabled(config.telemetry.per_actor_key.is_enabled());
        self.permissions.store(perm);
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

/// Replaces the current trace id with the provided one
/// if inside the actor system.
///
/// Returns `true` if the trace id has been replaced.
#[inline]
pub fn try_set_trace_id(trace_id: TraceId) -> bool {
    try_with(|scope| scope.set_trace_id(trace_id)).is_some()
}

/// Returns the current object's meta.
///
/// # Panics
/// This function will panic if called ouside the actor system.
#[inline]
pub fn meta() -> Arc<ActorMeta> {
    with(|scope| scope.meta().clone())
}

/// Returns the current object's meta if inside the actor system.
#[inline]
pub fn try_meta() -> Option<Arc<ActorMeta>> {
    try_with(|scope| scope.meta().clone())
}

thread_local! {
    static SERDE_MODE: Cell<SerdeMode> = Cell::new(SerdeMode::Normal);
}

/// A mode of (de)serialization.
/// Useful to alternate a behavior depending on a context.
#[stability::unstable]
#[derive(Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SerdeMode {
    /// A default mode, regular ser/de calls.
    Normal,
    /// Serialzation for dumping purposes.
    Dumping,
    // Network
}

/// Sets the specified serde mode and runs the function.
///
/// # Panics
/// If the provided function panics.
#[stability::unstable]
#[inline]
pub fn with_serde_mode<R>(mode: SerdeMode, f: impl FnOnce() -> R) -> R {
    // We use a guard here to restore the current serde mode even on panics.
    struct Guard(SerdeMode);
    impl Drop for Guard {
        fn drop(&mut self) {
            SERDE_MODE.with(|cell| cell.set(self.0));
        }
    }

    let mode = SERDE_MODE.with(|cell| cell.replace(mode));
    let _guard = Guard(mode);
    f()
}

/// Returns the current serde mode.
#[stability::unstable]
#[inline]
pub fn serde_mode() -> SerdeMode {
    SERDE_MODE.with(Cell::get)
}

#[test]
fn serde_mode_works() {
    #[derive(serde::Serialize)]
    struct S {
        #[serde(serialize_with = "crate::dumping::hide")]
        f: u32,
    }

    let value = S { f: 42 };

    // `Normal` mode
    assert_eq!(serde_json::to_string(&value).unwrap(), r#"{"f":42}"#);

    // `Dumping` mode
    let json = with_serde_mode(SerdeMode::Dumping, || {
        serde_json::to_string(&value).unwrap()
    });
    assert_eq!(json, r#"{"f":"<hidden>"}"#);

    // Restored `Normal` mode
    assert_eq!(serde_json::to_string(&value).unwrap(), r#"{"f":42}"#);

    // `Normal` mode must be restored after panic
    let res = std::panic::catch_unwind(|| with_serde_mode(SerdeMode::Dumping, || panic!("oops")));
    assert!(res.is_err());
    assert_eq!(serde_json::to_string(&value).unwrap(), r#"{"f":42}"#);
}
