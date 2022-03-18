use std::sync::Arc;

use tokio::runtime::Handle;

use crate::actor::ActorMeta;

pub(crate) trait RuntimeFilter: Fn(&ActorMeta) -> bool + Send + Sync + 'static {}
impl<F: Fn(&ActorMeta) -> bool + Send + Sync + 'static> RuntimeFilter for F {}

// === RuntimeManager ===

#[derive(Default, Clone)]
pub(crate) struct RuntimeManager {
    dedicated: Vec<(Arc<dyn RuntimeFilter>, Handle)>,
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
}
