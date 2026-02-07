use std::sync::{Arc, OnceLock};

use super::Dump;

static MAKE_RECORDER: OnceLock<MakeRecorder> = OnceLock::new();

type MakeRecorder = Box<dyn Fn(&'static str) -> Arc<dyn Recorder> + Sync + Send>;

#[instability::unstable]
pub trait Recorder: Send + Sync {
    fn enabled(&self) -> bool;
    fn record(&self, dump: Dump);
}

#[instability::unstable]
pub fn set_make_recorder(make_recorder: MakeRecorder) -> bool {
    MAKE_RECORDER.set(make_recorder).is_ok()
}

pub(crate) fn make_recorder(class: &'static str) -> Option<Arc<dyn Recorder>> {
    MAKE_RECORDER.get().map(|make| make(class))
}
