use std::sync::Arc;

use metrics::GaugeValue;
use seqlock::SeqLock;

use super::MetricKind;
use crate::protocol::GaugeEpoch;

// Sharded gauges are tricky to implement.
// Concurrent actors can update the same gauge in parallel either by using
// deltas (increment/decrement) or by setting an absolute value.
// In case of deltas, it should behaves like a counter.
//
// The main idea is to share the last absolute value with its epoch between
// all shards of the same gauge. Everytime the gauge is updated by setting
// an absolute value, the epoch is incremented, so all other shards can
// reset their cumulative deltas.
pub(crate) struct Gauge {
    origin: Arc<GaugeOrigin>,
    epoch: GaugeEpoch,
    delta: f64,
}

impl MetricKind for Gauge {
    type Output = (f64, GaugeEpoch);
    type Shared = Arc<GaugeOrigin>;
    type Value = GaugeValue;

    fn new(origin: Self::Shared) -> Self {
        Self {
            origin: origin.clone(),
            epoch: 0,
            delta: 0.0,
        }
    }

    fn update(&mut self, value: Self::Value) {
        let delta = match value {
            GaugeValue::Absolute(value) => {
                // This is the only place where the contention is possible.
                // However, it's rare, because the absolute value is usually
                // set only by one thread at a time.
                self.epoch = self.origin.set(value);
                self.delta = 0.0;
                return;
            }
            GaugeValue::Increment(delta) => delta,
            GaugeValue::Decrement(delta) => -delta,
        };

        let current_epoch = self.origin.get().1;

        if self.epoch == current_epoch {
            // The new epoch is the same as the current one, just accumulate the delta.
            self.delta += delta;
        } else {
            // The new epoch is already set by another shard, reset the delta.
            self.epoch = current_epoch;
            self.delta = delta;
        }
    }

    // NOTE: Shards are merged one by one without blocking the whole storage,
    // thus while executing this method for one specific shard, other shards
    // are still available for updates, the same as the shared state (origin).
    //
    // However, all shards are merged consecutively.
    fn merge(self, (out_value, out_epoch): &mut Self::Output) -> usize {
        let (last_absolute, current_epoch) = self.origin.get();

        // The epoch is always monotonically increasing.
        debug_assert!(current_epoch >= *out_epoch);
        debug_assert!(current_epoch >= self.epoch);

        if current_epoch > *out_epoch {
            *out_value = last_absolute;
            *out_epoch = current_epoch;
        }

        if current_epoch == self.epoch {
            *out_value += self.delta;
        }

        // It's inaccurate because the same origin can be accounted multiple times.
        std::mem::size_of::<GaugeOrigin>()
    }
}

#[derive(Default)]
pub(crate) struct GaugeOrigin(SeqLock<(f64, GaugeEpoch)>);

impl GaugeOrigin {
    fn get(&self) -> (f64, GaugeEpoch) {
        self.0.read()
    }

    fn set(&self, value: f64) -> GaugeEpoch {
        let mut pair = self.0.lock_write();
        let new_epoch = pair.1 + 1;
        *pair = (value, new_epoch);
        new_epoch
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::VecDeque, ops::Range};

    use proptest::prelude::*;

    use super::*;

    const ACTIONS: Range<usize> = 1..1000;
    const SHARDS: usize = 3;

    #[derive(Debug, Clone)]
    enum Action {
        Update(Update),
        // How many shards to merge (max `SHARDS`).
        // It emulates the real-world scenario when shards are merged one by one,
        // see `Gauge::merge` for details about the non-blocking merge.
        Merge(usize),
    }

    #[derive(Debug, Clone)]
    struct Update {
        shard: usize, // max `SHARDS - 1`
        value: GaugeValue,
    }

    fn action_strategy() -> impl Strategy<Value = Action> {
        // Use integers here to avoid floating point errors.
        prop_oneof![
            1 => (1..=SHARDS).prop_map(Action::Merge),
            10 => update_strategy().prop_map(Action::Update),
        ]
    }

    prop_compose! {
        fn update_strategy()(shard in 0..SHARDS, value in gauge_value_strategy()) -> Update {
            Update { shard, value }
        }
    }

    fn gauge_value_strategy() -> impl Strategy<Value = GaugeValue> {
        // Use integers here to avoid floating point errors.
        prop_oneof![
            1 => (0..10).prop_map(|v| GaugeValue::Absolute(v as f64)),
            5 => (1..10).prop_map(|v| GaugeValue::Increment(v as f64)),
            5 => (1..10).prop_map(|v| GaugeValue::Decrement(v as f64)),
        ]
    }

    proptest! {
        #[test]
        fn linearizability(actions in prop::collection::vec(action_strategy(), ACTIONS)) {
            let origin = Arc::new(GaugeOrigin::default());

            let mut shards = (0..SHARDS).map(|_| Gauge::new(origin.clone())).collect::<VecDeque<_>>();
            let mut expected = 0.0;
            let mut actual = (0.0, 0);

            for action in actions {
                match action {
                    Action::Update(update) => {
                        expected = update.value.update_value(expected);
                        shards[update.shard].update(update.value);
                    }
                    Action::Merge(limit) => {
                        for _ in 0..limit {
                            let shard = shards.pop_front().unwrap();
                            shard.merge(&mut actual);
                            shards.push_back(Gauge::new(origin.clone()));
                            assert_eq!(shards.len(), SHARDS);
                        }

                        // Check eventually consistency.
                        if limit == SHARDS {
                            prop_assert_eq!(actual.0, expected);
                        }
                    }
                }
            }

            // Check eventually consistency.
            for shard in shards {
                shard.merge(&mut actual);
            }
            prop_assert_eq!(actual.0, expected);
        }
    }
}
