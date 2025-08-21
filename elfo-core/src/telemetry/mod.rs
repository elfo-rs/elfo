use std::sync::Arc;

use once_cell::sync::OnceCell; // TODO: move to std

pub mod config;

static RECORDER: OnceCell<Arc<dyn Recorder>> = OnceCell::new();

pub trait Recorder: Send + Sync + 'static {
    fn enter(&self);
    fn exit(&self);
}

pub fn set_recorder(recorder: Arc<impl Recorder>) -> bool {
    RECORDER.set(recorder as _).is_ok()
}

#[inline]
pub(crate) fn enter() {
    if let Some(r) = RECORDER.get() {
        r.enter();
    }
}

#[inline]
pub(crate) fn exit() {
    if let Some(r) = RECORDER.get() {
        r.exit();
    }
}
