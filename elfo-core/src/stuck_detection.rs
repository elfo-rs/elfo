use std::{
    sync::Arc,
    thread::{self, Thread, ThreadId},
};

use derive_more::Constructor;
use fxhash::FxHashMap;
use parking_lot::Mutex;
use thread_local::ThreadLocal;

use elfo_utils::ward;

use crate::{actor::ActorMeta, scope};

/// Detects stuck actors.
/// Usually, `on_thread_park` is good place to call [`StuckDetector::check()`].
#[derive(Default, Clone)]
pub struct StuckDetector(Arc<Inner>);

/// Information about a stuck actor, returned by [`StuckDetector::check()`].
#[derive(Constructor)]
pub struct StuckActorInfo {
    meta: Arc<ActorMeta>,
    thread: Thread,
}

impl StuckActorInfo {
    /// Returns the actor's meta.
    pub fn meta(&self) -> &Arc<ActorMeta> {
        &self.meta
    }

    /// Returns a thread where the actor runs.
    pub fn thread(&self) -> &Thread {
        &self.thread
    }
}

type Epoch = u32;

#[derive(Default)]
struct Inner {
    tls: ThreadLocal<Mutex<PerThread>>,
    last_check: Mutex<FxHashMap<ThreadId, (Arc<ActorMeta>, Epoch)>>,
}

struct PerThread {
    thread: Thread,
    meta: Option<Arc<ActorMeta>>,
    epoch: Epoch,
}

impl PerThread {
    fn current() -> Self {
        Self {
            thread: thread::current(),
            meta: None,
            epoch: 0,
        }
    }
}

impl StuckDetector {
    pub(crate) fn enter(&self) {
        let slot = self.0.tls.get_or(|| Mutex::new(PerThread::current()));
        let mut item = slot.lock();

        // `ThreadLocal` can reuse the slot, so we need to reset it explicitly.
        if item.thread.id() != thread::current().id() {
            *item = PerThread::current();
        }

        item.meta = scope::try_meta();
        item.epoch = item.epoch.wrapping_add(1);
    }

    pub(crate) fn exit(&self) {
        let slot = ward!(self.0.tls.get());
        let mut item = slot.lock();
        item.meta = None;
    }

    /// Returns actors that run at the same thread since the last call.
    pub fn check(&self) -> impl Iterator<Item = StuckActorInfo> {
        let mut last_check = self.0.last_check.lock();
        let mut check = FxHashMap::with_capacity_and_hasher(last_check.len(), Default::default());
        let mut stuck = Vec::new();

        for slot in &self.0.tls {
            let current = slot.lock();
            let current_meta = ward!(&current.meta, continue);
            let thread_id = current.thread.id();

            check.insert(thread_id, (current_meta.clone(), current.epoch));

            let (prev_meta, prev_epoch) = ward!(last_check.get(&thread_id), continue);
            if current.epoch == *prev_epoch && current_meta == prev_meta {
                stuck.push(StuckActorInfo::new(
                    current_meta.clone(),
                    current.thread.clone(),
                ));
            }
        }

        *last_check = check;

        stuck.into_iter()
    }
}
