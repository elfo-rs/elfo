use std::sync::atomic::{AtomicU64, Ordering};

use elfo_utils::time::Instant;

use super::trace_id::{TraceId, TraceIdLayout, TruncatedTime};
use crate::addr::NodeNo;

// === ChunkRegistry ===

pub(crate) type ChunkRegistry = AtomicU64;
#[cold]
fn next_chunk(chunk_registry: &ChunkRegistry) -> u32 {
    let chunk_no = chunk_registry.fetch_add(1, Ordering::Relaxed);
    chunk_no as u32 & 0xfff
}

// === Generator ===

pub(crate) struct Generator {
    node_no: Option<NodeNo>,
    timestamp: CachedTruncatedTime,
    chunk_no: u32,
    counter: u32,
}

impl Default for Generator {
    fn default() -> Self {
        Self {
            node_no: None,
            timestamp: CachedTruncatedTime::now(),
            chunk_no: 0, // will be set on first `generate()` call
            counter: 0x3ff,
        }
    }
}

impl Generator {
    /// Generates a new trace id according to the next layout:
    /// * 1  bit  0 (zero)
    /// * 25 bits timestamp in secs
    /// * 16 bits node_no
    /// * 12 bits (chunk_no & 0xfff)
    /// * 10 bits counter
    pub(crate) fn generate(&mut self, chunk_registry: &ChunkRegistry) -> TraceId {
        // Check whether the chunk is exhausted.
        if self.counter == 0x3ff {
            self.chunk_no = next_chunk(chunk_registry);
            self.counter = 0;
        }

        self.counter += 1;
        let bottom = self.chunk_no << 10 | self.counter;

        // Lazily cache the node number from the scope if not set yet.
        // We do it here instead of `Default::default()` in order to support
        // trace ID generation before entering the actor system.
        //
        // If called outside of the actor system and the node number is unknown,
        // all generated trace IDs will have zero in `node_no` part.
        if self.node_no.is_none() {
            self.node_no = node_no();
        }

        TraceId::from_layout(TraceIdLayout {
            timestamp: self.timestamp.get(),
            node_no: self.node_no,
            bottom: bottom.into(),
        })
    }
}

#[cold]
fn node_no() -> Option<NodeNo> {
    crate::scope::try_node_no()
}

// === CachedTruncatedTime ===

pub(crate) struct CachedTruncatedTime {
    cached: TruncatedTime,
    when: Instant,
}

impl CachedTruncatedTime {
    const NANOS_PER_SEC: u64 = 1_000_000_000;

    fn now() -> Self {
        Self {
            cached: TruncatedTime::now(),
            when: Instant::now(),
        }
    }

    fn get(&mut self) -> TruncatedTime {
        let mono = Instant::now();

        if mono.nanos_since(self.when) >= Self::NANOS_PER_SEC {
            self.cached = TruncatedTime::now();
            self.when = mono;
        }

        self.cached
    }
}

#[cfg(test)]
mod tests {
    use std::{sync::Arc, time::Duration};

    use crate::{scope::Scope, ActorMeta, Addr};

    use super::*;

    #[test]
    fn smoke() {
        fn is_divisible_by_chunk(diff: u64) {
            assert_eq!((diff / (1 << 10)) * (1 << 10), diff);
        }

        elfo_utils::time::with_mock(|mock| {
            let chunk_registry = Arc::new(ChunkRegistry::default());
            let mut generator = Generator::default();

            let sec = 1 << 38;
            let st = u64::from(generator.generate(&chunk_registry));

            mock.advance(Duration::from_millis(500));
            assert_eq!(u64::from(generator.generate(&chunk_registry)), st + 1);
            mock.advance(Duration::from_millis(500));
            assert_eq!(u64::from(generator.generate(&chunk_registry)), st + sec + 2,);
            mock.advance(Duration::from_millis(500));
            assert_eq!(u64::from(generator.generate(&chunk_registry)), st + sec + 3);
            mock.advance(Duration::from_millis(500));
            assert_eq!(
                u64::from(generator.generate(&chunk_registry)),
                st + 2 * sec + 4
            );

            let chunk_registry1 = chunk_registry.clone();
            std::thread::spawn(move || {
                elfo_utils::time::with_mock(|mock| {
                    mock.advance(Duration::from_secs(2));
                    let mut generator = Generator::default();
                    is_divisible_by_chunk(
                        u64::from(generator.generate(&chunk_registry1)) - (st + 2 * sec),
                    );
                });
            })
            .join()
            .unwrap();

            for i in 5..1023 {
                assert_eq!(
                    u64::from(generator.generate(&chunk_registry)),
                    st + 2 * sec + i
                );
            }

            is_divisible_by_chunk(u64::from(generator.generate(&chunk_registry)) - (st + 2 * sec));
        });
    }

    #[test]
    fn node_no() {
        let chunk_registry = Arc::new(ChunkRegistry::default());
        let mut generator = Generator::default();
        assert_eq!(
            generator.generate(&chunk_registry).to_layout().node_no,
            None
        );

        let scope = Scope::test(
            Addr::NULL,
            ActorMeta {
                group: "test".into(),
                key: "_".into(),
            }
            .into(),
        );

        scope.sync_within(|| {
            assert_ne!(
                generator.generate(&chunk_registry).to_layout().node_no,
                None,
            );
        });
    }
}
