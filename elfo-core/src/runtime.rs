use std::sync::Arc;

use tokio::runtime::Handle;

use crate::actor::ActorMeta;
#[cfg(feature = "unstable-stuck-detection")]
use crate::stuck_detection::StuckDetector;

pub(crate) trait RuntimeFilter: Fn(&ActorMeta) -> bool + Send + Sync + 'static {}
impl<F: Fn(&ActorMeta) -> bool + Send + Sync + 'static> RuntimeFilter for F {}

// === RuntimeManager ===

#[derive(Default, Clone)]
pub(crate) struct RuntimeManager {
    dedicated: Vec<(Arc<dyn RuntimeFilter>, Handle)>,
    #[cfg(feature = "unstable-stuck-detection")]
    stuck_detector: StuckDetector,
}

impl RuntimeManager {
    pub(crate) fn add<F: RuntimeFilter>(&mut self, filter: F, handle: Handle) {
        self.dedicated.push((Arc::new(filter), handle));
    }

    pub(crate) fn get(&self, meta: &ActorMeta) -> Handle {
        for (f, h) in &self.dedicated {
            if f(meta) {
                return h.clone();
            }
        }

        tokio::runtime::Handle::current()
    }

    #[cfg(feature = "unstable-stuck-detection")]
    pub(crate) fn stuck_detector(&self) -> StuckDetector {
        self.stuck_detector.clone()
    }
}
