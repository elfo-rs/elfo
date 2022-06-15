use std::{
    collections::VecDeque,
    mem,
    sync::Arc,
    time::{Duration, Instant},
};

use fxhash::{FxHashMap, FxHashSet};
use parking_lot::{Mutex, MutexGuard};
use thread_local::ThreadLocal;

use elfo_core::dumping::Dump;
use elfo_utils::CachePadded;

type ShardNo = usize;

#[derive(Debug, Clone)]
struct DumpRegistryConfig {
    max_part_count: usize,
}

// === DumpStorage ===

pub(crate) struct DumpStorage {
    registry_config: DumpRegistryConfig,
    registries: FxHashMap<&'static str, Arc<DumpRegistry>>,
    classes: FxHashSet<&'static str>,
}

impl DumpStorage {
    pub(crate) fn new() -> Self {
        Self {
            registry_config: DumpRegistryConfig {
                // At startup we doesn't limit dumping at all.
                // The dumper reconfigure the storage at startup.
                max_part_count: usize::max_value(),
            },
            registries: Default::default(),
            classes: Default::default(),
        }
    }

    pub(crate) fn configure(&mut self, registry_capacity: usize) {
        self.registry_config = DumpRegistryConfig {
            max_part_count: registry_capacity / PART_CAPACITY,
        };

        for registry in self.registries.values() {
            registry.configure(self.registry_config.clone());
        }
    }

    pub(crate) fn registry(&mut self, class: &'static str) -> Arc<DumpRegistry> {
        let config = self.registry_config.clone();
        self.classes.insert(class);
        self.registries
            .entry(class)
            .or_insert_with(|| Arc::new(DumpRegistry::new(class, config)))
            .clone()
    }

    pub(crate) fn classes(&self) -> &FxHashSet<&'static str> {
        &self.classes
    }
}

// === DumpRegistry ===

pub(crate) struct DumpRegistry {
    class: &'static str,
    fund: Mutex<Fund>,
    shards: ThreadLocal<Shard>,
}

struct Shard {
    shard_no: ShardNo,
    active_part: CachePadded<Mutex<Part>>,
}

impl DumpRegistry {
    fn new(class: &'static str, config: DumpRegistryConfig) -> Self {
        Self {
            class,
            fund: Mutex::new(Fund::new(config)),
            shards: Default::default(),
        }
    }

    pub(crate) fn class(&self) -> &'static str {
        self.class
    }

    pub(crate) fn add(&self, dump: Dump) {
        let shard = self.shards.get_or(|| self.make_shard());
        let need_to_renew = {
            let mut active_part = shard.active_part.lock();
            active_part.push(dump);
            active_part.is_full()
        };

        if need_to_renew {
            self.renew_active_part(shard, Part::is_full);
        }
    }

    pub(crate) fn drain(&self, timeout: Duration) -> Drain<'_> {
        Drain::new(self, timeout)
    }

    fn configure(&self, config: DumpRegistryConfig) {
        self.fund.lock().configure(config);
    }

    fn fund(&self) -> MutexGuard<'_, Fund> {
        self.fund.lock()
    }

    fn shards(&self) -> thread_local::Iter<'_, Shard> {
        self.shards.iter()
    }

    #[cold]
    #[inline(never)]
    fn make_shard(&self) -> Shard {
        let mut fund = self.fund.lock();
        let shard_no = fund.add_shard();
        Shard {
            shard_no,
            active_part: CachePadded(Mutex::new(fund.get_empty_part())),
        }
    }

    #[cold]
    #[inline(never)]
    fn renew_active_part(&self, shard: &Shard, predicate: impl FnOnce(&Part) -> bool) -> bool {
        let mut fund = self.fund.lock();

        if !predicate(&shard.active_part.lock()) {
            return false;
        }

        let empty_part = fund.get_empty_part();
        let active_part = mem::replace(&mut *shard.active_part.lock(), empty_part);
        fund.add_filled_part(shard.shard_no, active_part);
        true
    }
}

// === Fund ===

struct Fund {
    config: DumpRegistryConfig,
    // Empty + filled + active + used by `Drain`.
    part_count: usize,
    empty_parts: Vec<Part>,
    filled_parts: Vec<VecDeque<Part>>,
}

impl Fund {
    fn new(config: DumpRegistryConfig) -> Self {
        Self {
            config,
            part_count: 0,
            empty_parts: Vec::with_capacity(128),
            filled_parts: Vec::with_capacity(64),
        }
    }

    fn configure(&mut self, config: DumpRegistryConfig) {
        while self.part_count > config.max_part_count && self.empty_parts.pop().is_some() {
            self.part_count -= 1;
        }

        while self.part_count > config.max_part_count && self.clear_most_filled().is_some() {
            self.part_count -= 1;
        }

        self.config = config;
    }

    fn add_shard(&mut self) -> ShardNo {
        let shard_no = self.filled_parts.len();
        self.filled_parts.push(Default::default());
        shard_no
    }

    fn add_filled_part(&mut self, shard_no: ShardNo, part: Part) {
        debug_assert!(!part.is_empty());
        self.filled_parts[shard_no].push_back(part);
    }

    fn get_filled_part(&mut self, shard_no: ShardNo) -> Option<Part> {
        self.filled_parts[shard_no].pop_front()
    }

    fn add_empty_part(&mut self, part: Part) {
        debug_assert!(part.is_empty());
        self.empty_parts.push(part);
    }

    fn get_empty_part(&mut self) -> Part {
        // Firstly, check the empty list.
        if let Some(part) = self.empty_parts.pop() {
            debug_assert!(part.is_empty());
            return part;
        }

        // If we have already reached the limit, try to find the most filled shard
        // and take his oldest part.
        if self.part_count >= self.config.max_part_count {
            if let Some(part) = self.clear_most_filled() {
                return part;
            }
        }

        // Otherwise, we have only active parts or the limit hasn't reached,
        // so allocate a new part.
        self.part_count += 1;
        Part::new()
    }

    fn clear_most_filled(&mut self) -> Option<Part> {
        let candidate = self.filled_parts.iter_mut().max_by_key(|q| q.len())?;
        let mut part = candidate.pop_front()?;
        // TODO: count lost.
        part.clear();
        Some(part)
    }
}

// === Part ===

// -1 since the `VecDeque` always leaves one space empty.
const PART_CAPACITY: usize = 8191; // must be `N^2 - 1`

struct Part {
    items: VecDeque<Dump>,
}

impl Part {
    fn new() -> Self {
        let items = VecDeque::with_capacity(PART_CAPACITY);
        debug_assert_eq!(items.capacity(), PART_CAPACITY);
        Self { items }
    }

    fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    fn is_full(&self) -> bool {
        self.items.len() == PART_CAPACITY
    }

    fn push(&mut self, dump: Dump) {
        self.items.push_back(dump);
    }

    fn pop(&mut self) -> Option<Dump> {
        self.items.pop_front()
    }

    fn clear(&mut self) {
        self.items.clear();
    }
}

// === Drain ===

/// A consuming iterator over dumps.
///
/// It iterates over shards and takes no more than one part per shard in a row
/// to ensure fairness. The iterator returns `None` if all shards are empty or
/// a specified timeout is reached.
pub(crate) struct Drain<'a> {
    registry: &'a DumpRegistry,
    shard_iter: thread_local::Iter<'a, Shard>,
    current: Option<(&'a Shard, Part)>,
    until: Instant,
}

impl<'a> Drain<'a> {
    fn new(registry: &'a DumpRegistry, timeout: Duration) -> Self {
        Drain {
            registry,
            shard_iter: registry.shards(),
            current: None,
            until: Instant::now() + timeout,
        }
    }

    fn next_shard(&mut self) {
        debug_assert!(self.current.is_none());

        // We should rerun the shard iterator if it ends, but only once.
        let mut is_rerunned = false;

        self.current = Some(loop {
            let shard = match self.shard_iter.next() {
                Some(shard) => shard,
                None if is_rerunned => return,
                None => {
                    is_rerunned = true;
                    self.shard_iter = self.registry.shards();
                    continue;
                }
            };

            // Firstly, check filled parts.
            if let Some(part) = self.registry.fund().get_filled_part(shard.shard_no) {
                break (shard, part);
            }

            // Then, try to renew an active part if it's not empty.
            if !self
                .registry
                .renew_active_part(shard, |part| !part.is_empty())
            {
                continue;
            }

            // If the active part has been renewed, check filled parts again.
            // Theoretically, this call still can return `None`.
            if let Some(part) = self.registry.fund().get_filled_part(shard.shard_no) {
                break (shard, part);
            }
        });
    }
}

impl<'a> Iterator for Drain<'a> {
    type Item = Dump;

    fn next(&mut self) -> Option<Dump> {
        if self.current.is_none() && Instant::now() < self.until {
            self.next_shard();
        }

        let (_, part) = self.current.as_mut()?;
        let item = part.pop();
        debug_assert!(item.is_some());

        if part.is_empty() {
            let (_, part) = self.current.take().unwrap();
            self.registry.fund().add_empty_part(part);
        }

        item
    }
}

impl Drop for Drain<'_> {
    fn drop(&mut self) {
        if let Some((shard, part)) = self.current.take() {
            self.registry.fund().add_filled_part(shard.shard_no, part);
        }
    }
}

#[cfg(TODO)] // TODO
#[cfg(test)]
#[cfg(feature = "test-util")]
mod tests {
    use std::convert::TryFrom;

    use smallbox::smallbox;
    use tokio::time;

    use super::*;
    use crate::{actor::ActorMeta, dumping::SequenceNo, scope::Scope, trace_id::TraceId, Addr};

    fn dump_msg(dumper: &Dumper, name: &'static str) {
        dumper.dump(
            Direction::In,
            "class",
            name,
            "proto",
            MessageKind::Regular,
            smallbox!(42),
        );
    }

    #[tokio::test(start_paused = true)]
    async fn it_works() {
        let meta = Arc::new(ActorMeta {
            group: "group".into(),
            key: "key".into(),
        });
        let trace_id = TraceId::try_from(42).unwrap();

        let f = async {
            let dumper = Dumper::default();
            let mut drain = dumper.drain(Duration::from_secs(10));

            assert!(drain.next().is_none());
            assert!(drain.next().is_none());

            dump_msg(&dumper, "1");

            let msg = drain.next().unwrap();
            assert_eq!(msg.meta, meta);
            assert_eq!(msg.sequence_no, SequenceNo::try_from(1).unwrap());
            assert_eq!(msg.timestamp, Timestamp::from_nanos(42));
            assert_eq!(msg.trace_id, trace_id);
            assert_eq!(msg.direction, Direction::In);
            assert_eq!(msg.class, "class");
            assert_eq!(msg.message_name, "1");
            assert_eq!(msg.message_protocol, "proto");
            assert_eq!(msg.message_kind, MessageKind::Regular);

            assert!(drain.next().is_none());

            time::advance(time::Duration::new(0, 100)).await;

            dump_msg(&dumper, "2");
            dump_msg(&dumper, "3");

            let msg = drain.next().unwrap();
            assert_eq!(msg.sequence_no, SequenceNo::try_from(2).unwrap());
            assert_eq!(msg.timestamp, Timestamp::from_nanos(42));
            assert_eq!(msg.message_name, "2");
            let msg = drain.next().unwrap();
            assert_eq!(msg.message_name, "3");

            assert!(drain.next().is_none());
        };

        let scope = Scope::test(Addr::NULL, meta.clone());
        scope.set_trace_id(trace_id);
        scope.within(f).await;
    }
}
