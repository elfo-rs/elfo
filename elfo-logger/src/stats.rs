use metrics::{Key, Label};
use tracing::Level;

use elfo_utils::CachePadded;

struct KeysPerLevel([Key; 5]);

impl KeysPerLevel {
    const fn new(name: &'static str) -> Self {
        const fn labels(value: &'static str) -> [Label; 1] {
            [Label::from_static_parts("level", value)]
        }

        const TRACE: &[Label] = &labels("Trace");
        const DEBUG: &[Label] = &labels("Debug");
        const INFO: &[Label] = &labels("Info");
        const WARN: &[Label] = &labels("Warn");
        const ERROR: &[Label] = &labels("Error");

        Self([
            Key::from_static_parts(name, TRACE),
            Key::from_static_parts(name, DEBUG),
            Key::from_static_parts(name, INFO),
            Key::from_static_parts(name, WARN),
            Key::from_static_parts(name, ERROR),
        ])
    }

    fn get(&self, level: Level) -> &Key {
        let index = match level {
            Level::TRACE => 0,
            Level::DEBUG => 1,
            Level::INFO => 2,
            Level::WARN => 3,
            Level::ERROR => 4,
        };

        &self.0[index]
    }
}

struct Stats {
    emitted: KeysPerLevel,
    lost: KeysPerLevel,
    limited: KeysPerLevel,
}

static STATS: CachePadded<Stats> = CachePadded::new(Stats {
    emitted: KeysPerLevel::new("elfo_emitted_events_total"),
    lost: KeysPerLevel::new("elfo_lost_events_total"),
    limited: KeysPerLevel::new("elfo_limited_events_total"),
});

fn increment(keys: &KeysPerLevel, level: Level) {
    let recorder = ward!(metrics::try_recorder());
    recorder.increment_counter(keys.get(level), 1);
}

pub(crate) fn on_emitted_event(level: Level) {
    increment(&STATS.emitted, level);
}

pub(crate) fn on_lost_event(level: Level) {
    increment(&STATS.lost, level);
}

pub(crate) fn on_limited_event(level: Level) {
    increment(&STATS.limited, level);
}
