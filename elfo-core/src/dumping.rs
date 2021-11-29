use std::{
    collections::VecDeque,
    mem,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};

use metrics::increment_counter;
use parking_lot::Mutex;
use serde::Deserialize;
use smallbox::smallbox;
use tokio::time::Instant;
use tracing::error;

use elfo_utils::{CachePadded, RateLimiter};

#[allow(unreachable_pub)] // Actually, it's reachable via `elfo::_priv`.
pub use self::{dump_item::*, sequence_no::SequenceNo};

pub use hider::hide;
pub(crate) use hider::set_in_dumping;

use self::sequence_no::SequenceNoGenerator;
use crate::{
    envelope,
    message::{Message, Request},
    request_table::RequestId,
    scope,
};

mod dump_item;
mod hider;
mod sequence_no;

const SHARD_COUNT: usize = 16;
const SHARD_MAX_LEN: usize = 300_000;

static NEXT_SHARD_NO: AtomicUsize = AtomicUsize::new(0);
thread_local! {
    static SHARD_NO: usize = NEXT_SHARD_NO.fetch_add(1, Ordering::Relaxed) % SHARD_COUNT;
}

// Reexported in `elfo::_priv`.
#[derive(Clone, Default)]
#[doc(hidden)]
pub struct Dumper {
    per_system: Arc<PerSystem>,
    per_group: Arc<PerGroup>,
}

#[derive(Deserialize)]
#[serde(default)]
pub(crate) struct DumpingConfig {
    disabled: bool,
    max_rate: u64,
}

impl Default for DumpingConfig {
    fn default() -> Self {
        Self {
            disabled: false,
            max_rate: 100_000,
        }
    }
}

#[derive(Default)]
struct PerSystem {
    shards: [CachePadded<Mutex<VecDeque<DumpItem>>>; SHARD_COUNT],
}

#[derive(Default)]
struct PerGroup {
    // TODO: add `CachePadded`.
    rate_limiter: RateLimiter,
    sequence_no_gen: SequenceNoGenerator,
    is_possible: bool,
    is_disabled: AtomicBool,
}

impl Dumper {
    pub(crate) fn for_group(&self, is_possible: bool) -> Self {
        let mut specific = self.clone();
        specific.per_group = Arc::new(PerGroup {
            rate_limiter: RateLimiter::default(),
            sequence_no_gen: SequenceNoGenerator::default(),
            is_possible,
            is_disabled: AtomicBool::new(false),
        });
        specific
    }

    pub(crate) fn configure(&self, config: &DumpingConfig) {
        self.per_group
            .is_disabled
            .store(config.disabled, Ordering::Relaxed);
        self.per_group.rate_limiter.configure(config.max_rate);
    }

    #[inline]
    pub fn is_enabled(&self) -> bool {
        if !self.per_group.is_possible || self.per_group.is_disabled.load(Ordering::Relaxed) {
            return false;
        }

        if self.per_group.rate_limiter.acquire() {
            // TODO: increase `sequence_no` anyway.
            true
        } else {
            increment_counter!("elfo_limited_dumps_total");
            false
        }
    }

    #[inline(always)]
    pub(crate) fn dump_message<M: Message>(
        &self,
        message: &M,
        kind: &envelope::MessageKind,
        direction: Direction,
    ) {
        self.dump(
            direction,
            "",
            M::NAME,
            M::PROTOCOL,
            MessageKind::from_message_kind(kind),
            smallbox!(message.clone()),
        );
    }

    #[inline(always)]
    pub(crate) fn dump_response<R: Request>(
        &self,
        message: &R::Response,
        request_id: RequestId,
        direction: Direction,
    ) {
        use slotmap::Key;

        self.dump(
            direction,
            "",
            R::Wrapper::NAME,
            R::Wrapper::PROTOCOL,
            MessageKind::Response(request_id.data().as_ffi()),
            smallbox!(message.clone()),
        );
    }

    pub fn dump(
        &self,
        direction: Direction,
        class: &'static str,
        message_name: &'static str,
        message_protocol: &'static str,
        message_kind: MessageKind,
        message: ErasedMessage,
    ) {
        let d = scope::try_with(|scope| (scope.meta().clone(), scope.trace_id()));

        let (meta, trace_id) = ward!(d, {
            cooldown!(Duration::from_secs(15), {
                error!("attempt to dump outside the actor scope");
            });
            return;
        });

        let item = DumpItem {
            meta,
            sequence_no: self.per_group.sequence_no_gen.generate(),
            timestamp: Timestamp::now(),
            trace_id,
            direction,
            class,
            message_name,
            message_protocol,
            message_kind,
            message,
        };

        let shard_no = SHARD_NO.with(|shard_no| *shard_no);
        let mut queue = self.per_system.shards[shard_no].lock();

        if queue.len() >= SHARD_MAX_LEN {
            // TODO: move to a limited backlog.
            increment_counter!("elfo_lost_dumps_total");
            return;
        }

        increment_counter!("elfo_emitted_dumps_total");

        queue.push_back(item);
    }

    pub fn drain(&self, timeout: Duration) -> Drain<'_> {
        Drain {
            dumper: self,
            shard_no: 0,
            queue: VecDeque::new(),
            until: Instant::now() + timeout,
        }
    }
}

// Reexported in `elfo::_priv`.
#[doc(hidden)]
pub struct Drain<'a> {
    dumper: &'a Dumper,
    shard_no: usize,
    queue: VecDeque<DumpItem>,
    until: Instant,
}

impl<'a> Drain<'a> {
    fn refill_queue(&mut self) {
        debug_assert!(self.queue.is_empty());
        let mut next_shard_no = self.shard_no;

        #[allow(clippy::blocks_in_if_conditions)]
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
        } else if Instant::now() < self.until {
            self.refill_queue();
            self.queue.pop_front()
        } else {
            None
        }
    }
}

#[cfg(test)]
#[cfg(feature = "test-util")]
mod tests {
    use super::*;

    use std::convert::TryFrom;

    use smallbox::smallbox;
    use tokio::time;

    use crate::{actor::ActorMeta, addr::Addr, scope::Scope, trace_id::TraceId};

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

    #[tokio::test]
    async fn it_works() {
        time::pause();

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
            assert_eq!(msg.timestamp, Timestamp::from_nanos(42)); // TODO: improve the mock.
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
