use std::{cell::Cell, future::Future, sync::Arc};

use crate::{actor::ActorMeta, addr::Addr, trace_id::TraceId};

tokio::task_local! {
    static META: Arc<ActorMeta>;
    static TRACE_ID: Cell<TraceId>;
}

#[deprecated(note = "use `elfo::scope::trace_id()` instead")]
pub fn trace_id() -> TraceId {
    crate::scope::trace_id()
}

#[deprecated(note = "use `elfo::scope::try_trace_id()` instead")]
pub fn try_trace_id() -> Option<TraceId> {
    crate::scope::try_trace_id()
}

#[deprecated(note = "use `elfo::scope::set_trace_id()` instead")]
pub fn set_trace_id(trace_id: TraceId) {
    crate::scope::set_trace_id(trace_id);
}

#[deprecated(note = "use `elfo::scope::meta()` instead")]
pub fn meta() -> Arc<ActorMeta> {
    crate::scope::meta()
}

#[deprecated(note = "use `elfo::scope::try_meta()` instead")]
pub fn try_meta() -> Option<Arc<ActorMeta>> {
    crate::scope::try_meta()
}

#[deprecated(note = "use `elfo::scope` instead")]
pub async fn scope<F: Future>(meta: Arc<ActorMeta>, trace_id: TraceId, f: F) -> F::Output {
    #[allow(deprecated)]
    let scope = make_stupid_scope(meta);
    scope.set_trace_id(trace_id);
    scope.within(f).await
}

#[deprecated(note = "use `elfo::scope` instead")]
pub fn sync_scope<R>(meta: Arc<ActorMeta>, trace_id: TraceId, f: impl FnOnce() -> R) -> R {
    #[allow(deprecated)]
    let scope = make_stupid_scope(meta);
    scope.set_trace_id(trace_id);
    scope.sync_within(f)
}

fn make_stupid_scope(meta: Arc<ActorMeta>) -> crate::scope::Scope {
    crate::scope::Scope::new(
        Addr::NULL,
        Addr::NULL,
        meta,
        Default::default(),
        Default::default(),
    )
}
