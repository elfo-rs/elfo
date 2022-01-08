use std::sync::Arc;

use once_cell::sync::OnceCell;

use super::DumpItem;

static MAKE_RECORDER: OnceCell<MakeRecorder> = OnceCell::new();

type MakeRecorder = Box<dyn Fn(&'static str) -> Arc<dyn Recorder> + Sync + Send>;

#[stability::unstable]
pub trait Recorder: Send + Sync {
    fn enabled(&self) -> bool;
    fn record(&self, dump: DumpItem);
}

#[stability::unstable]
pub fn set_make_recorder(make_recorder: MakeRecorder) -> bool {
    MAKE_RECORDER.set(make_recorder).is_ok()
}

pub(crate) fn make_recorder(class: &'static str) -> Option<Arc<dyn Recorder>> {
    MAKE_RECORDER.get().map(|make| make(class))
}
