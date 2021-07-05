use std::{
    collections::VecDeque,
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

#[derive(Clone, Default)]
pub(crate) struct Dumper {
    per_system: Arc<PerSystem>,
    per_group: Arc<PerGroup>,
}

#[derive(Default)]
struct PerSystem {
    shards: [CachePadded<Mutex<VecDeque<DumpItem>>>; SHARD_COUNT],
}

#[derive(Default)]
struct PerGroup {
    sequence_no_gen: SequenceNoGenerator,
    filter: Filter,
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
            sequence_no_gen: SequenceNoGenerator::default(),
            filter,
        });
        specific
    }

    pub(crate) fn is_enabled(&self) -> bool {
        !matches!(self.per_group.filter, Filter::Nothing)
    }

    pub(crate) fn dump(
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
}
