use std::sync::{Arc, OnceLock};

pub mod config;

static RECORDER: OnceLock<Arc<dyn Recorder>> = OnceLock::new();

pub trait Recorder: Send + Sync + 'static {
    fn enter(&self);
    fn exit(&self);
}

pub fn set_recorder(recorder: Arc<impl Recorder>) -> bool {
    RECORDER.set(recorder as _).is_ok()
}

#[inline]
pub fn enter() {
    if let Some(r) = RECORDER.get() {
        r.enter();
    }
}

#[inline]
pub fn exit() {
    if let Some(r) = RECORDER.get() {
        r.exit();
    }
}
