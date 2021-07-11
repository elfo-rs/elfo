use std::{
    collections::VecDeque,
    mem,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};

use parking_lot::Mutex;
use serde::Serialize;
use smallbox::smallbox;

use elfo_utils::CachePadded;

#[allow(unreachable_pub)] // Actually, it's reachable via `elfo::_priv`.
pub use self::{dump_item::*, sequence_no::SequenceNo};

use self::sequence_no::SequenceNoGenerator;
use crate::{time::Timestamp, tls};

mod dump_item;
mod sequence_no;

const SHARD_COUNT: usize = 16;

static NEXT_SHARD_NO: AtomicUsize = AtomicUsize::new(0);
thread_local! {
    static SHARD_NO: usize = NEXT_SHARD_NO.fetch_add(1, Ordering::Relaxed) % SHARD_COUNT;
}

// Reexported in `elfo::_priv`.
#[derive(Clone, Default)]
pub struct Dumper {
    per_system: Arc<PerSystem>,
    per_group: Arc<PerGroup>,
}

#[derive(Default)]
struct PerSystem {
    shards: [CachePadded<Mutex<VecDeque<DumpItem>>>; SHARD_COUNT],
}

#[derive(Default)]
struct PerGroup {
    sequence_no_gen: CachePadded<SequenceNoGenerator>,
    filter: CachePadded<Filter>,
}

pub(crate) enum Filter {
    All,
    Nothing,
}

impl Default for Filter {
    fn default() -> Self {
        Self::Nothing
    }
}

impl Dumper {
    pub(crate) fn for_group(&self, filter: Filter) -> Self {
        let mut specific = self.clone();
        specific.per_group = Arc::new(PerGroup {
            sequence_no_gen: CachePadded(SequenceNoGenerator::default()),
            filter: CachePadded(filter),
        });
        specific
    }

    pub fn is_enabled(&self) -> bool {
        !matches!(self.per_group.filter.0, Filter::Nothing)
    }

    pub fn dump(
        &self,
        direction: Direction,
        class: &'static str,
        message_name: &'static str,
        message_protocol: &'static str,
        message_kind: MessageKind,
        message: impl Serialize + Send + 'static,
    ) {
        let item = DumpItem {
            meta: tls::meta(),
            sequence_no: self.per_group.sequence_no_gen.generate(),
            timestamp: Timestamp::now(),
            trace_id: tls::trace_id(),
            direction,
            class,
            message_name,
            message_protocol,
            message_kind,
            message: smallbox!(message),
        };

        let shard_no = SHARD_NO.with(|shard_no| *shard_no);
        let mut queue = self.per_system.shards[shard_no].lock();
        queue.push_back(item);
    }

    pub fn drain(&self) -> Drain<'_> {
        Drain {
            dumper: self,
            shard_no: 0,
            queue: VecDeque::new(),
        }
    }
}

// Reexported in `elfo::_priv`.
pub struct Drain<'a> {
    dumper: &'a Dumper,
    shard_no: usize,
    queue: VecDeque<DumpItem>,
}

impl<'a> Drain<'a> {
    fn refill_queue(&mut self) {
        debug_assert!(self.queue.is_empty());
        let mut next_shard_no = self.shard_no;

        while {
            {
                let mut next_queue = self.dumper.per_system.shards[next_shard_no].lock();
                mem::swap(&mut self.queue, &mut next_queue);
            }

            next_shard_no = (next_shard_no + 1) % SHARD_COUNT;
            self.queue.is_empty() && next_shard_no != self.shard_no
        } {}

        self.shard_no = next_shard_no;
    }
}

impl<'a> Iterator for Drain<'a> {
    type Item = DumpItem;

    fn next(&mut self) -> Option<DumpItem> {
        if let Some(item) = self.queue.pop_front() {
            Some(item)
        } else {
            self.refill_queue();
            self.queue.pop_front()
        }
    }
}

#[cfg(test)]
#[cfg(feature = "test-util")]
mod tests {
    use super::*;

    use std::convert::TryFrom;

    use crate::{object::ObjectMeta, time, trace_id::TraceId};

    fn dump_msg(dumper: &Dumper, name: &'static str) {
        dumper.dump(
            Direction::In,
            "class",
            name,
            "proto",
            MessageKind::Regular,
            42,
        );
    }

    #[tokio::test]
    async fn it_works() {
        time::pause();

        let meta = Arc::new(ObjectMeta {
            group: "group".into(),
            key: Some("key".into()),
        });
        let trace_id = TraceId::try_from(42).unwrap();

        tls::scope(meta.clone(), trace_id, async {
            let dumper = Dumper::default();
            let mut drain = dumper.drain();

            assert!(drain.next().is_none());
            assert!(drain.next().is_none());

            dump_msg(&dumper, "1");

            let msg = drain.next().unwrap();
            assert_eq!(msg.meta, meta);
            assert_eq!(msg.sequence_no, SequenceNo::try_from(1).unwrap());
            assert_eq!(msg.timestamp, Timestamp::from_nanos(0));
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
            assert_eq!(msg.timestamp, Timestamp::from_nanos(100));
            assert_eq!(msg.message_name, "2");
            let msg = drain.next().unwrap();
            assert_eq!(msg.message_name, "3");

            assert!(drain.next().is_none());
        })
        .await;
    }
}
