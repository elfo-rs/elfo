use std::sync::atomic::{AtomicU64, Ordering};

use crate::dumping;

// Layout:
// ```text
// 64     16      7  6  5  4  3  2  1  0
//  ┌──────┬──────┬──┬──┬──┬──┬──┬──┬──┐
//  │DdDdDd│      │MK│MG│LE│LW│LI│LD│LT│
//  └──────┴──────┴──┴──┴──┴──┴──┴──┴──┘
//     │       │   │   │  │  │  │  │  └─── logging: Trace level
//     │       │   │   │  │  │  │  └────── logging: Debug level
//     │       │   │   │  │  │  └───────── logging: Info level
//     │       │   │   │  │  └──────────── logging: Warn level
//     │       │   │   │  └─────────────── logging: Error level
//     │       │   │   └────────────────── metrics: per actor group
//     │       │   └────────────────────── metrics: per actor key
//     │       └────────────────────────── <reserved>
//     └────────────────────────────────── dumping (2 bits per class):
//                                           0 = Off     2 = Verbose
//                                           1 = Normal  3 = Total
// ```
//
// Reexported in `elfo::_priv`.
#[derive(Default)]
pub struct AtomicPermissions(AtomicU64);

const LOGGING_MASK: u64 = 0b00011111;
const TELEMETRY_PER_ACTOR_GROUP_IS_ENABLED: u64 = 0b00100000;
const TELEMETRY_PER_ACTOR_KEY_IS_ENABLED: u64 = 0b01000000;
const DUMPING_CLASSES: u32 = 24;
const DUMPING_BITS_PER_CLASS: u32 = 2;
const DUMPING_CLASS_MASK: u64 = (1 << DUMPING_BITS_PER_CLASS) - 1;
const DUMPING_SHIFT: u32 = 64 - DUMPING_CLASSES * DUMPING_BITS_PER_CLASS;
const DUMPING_MASK: u64 = u64::MAX << DUMPING_SHIFT;

impl AtomicPermissions {
    pub(crate) fn store(&self, perm: Permissions) {
        // Now it's not required to use a stronger ordering.
        self.0.store(perm.0, Ordering::Relaxed)
    }

    pub(crate) fn load(&self) -> Permissions {
        // Now it's not required to use a stronger ordering.
        Permissions(self.0.load(Ordering::Relaxed))
    }
}

// Reexported in `elfo::_priv`.
#[derive(Debug, Clone, Copy, Default)]
pub struct Permissions(u64);

impl Permissions {
    #[inline]
    pub fn is_logging_enabled(&self, level: tracing::Level) -> bool {
        let mask = 1 << log_level_to_value(level);
        self.0 & mask != 0
    }

    #[inline]
    pub fn is_dumping_enabled(&self, class_no: u8, level: dumping::Level) -> bool {
        let shift = DUMPING_SHIFT + DUMPING_BITS_PER_CLASS * u32::from(class_no);
        let allowed = (self.0 >> shift) & DUMPING_CLASS_MASK;
        allowed as u32 >= level as u32
    }

    #[inline]
    pub fn is_telemetry_per_actor_group_enabled(&self) -> bool {
        self.0 & TELEMETRY_PER_ACTOR_GROUP_IS_ENABLED != 0
    }

    #[inline]
    pub fn is_telemetry_per_actor_key_enabled(&self) -> bool {
        self.0 & TELEMETRY_PER_ACTOR_KEY_IS_ENABLED != 0
    }

    /// `None` is to disable logging at all.
    pub(crate) fn set_logging_enabled(&mut self, max_level: Option<tracing::Level>) {
        self.0 &= !LOGGING_MASK;

        if let Some(max_level) = max_level.map(log_level_to_value) {
            let disabled = (1 << max_level) - 1;
            self.0 |= LOGGING_MASK & !disabled;
        }
    }

    pub(crate) fn set_dumping_enabled(
        &mut self,
        default: Option<dumping::Level>,
        overrides: impl IntoIterator<Item = (u8, Option<dumping::Level>)>,
    ) {
        self.0 &= !DUMPING_MASK;

        let mut bits = 0;

        // Fill with the default level.
        for _ in 0..DUMPING_CLASSES {
            bits |= dump_level_filter_to_value(default);
            bits <<= DUMPING_BITS_PER_CLASS;
        }

        // Override with the specified levels.
        for (class_no, level) in overrides.into_iter() {
            assert!(u32::from(class_no) <= DUMPING_CLASSES);
            let shift = DUMPING_BITS_PER_CLASS * u32::from(class_no);
            bits &= !(DUMPING_CLASS_MASK << shift);
            bits |= dump_level_filter_to_value(level) << shift;
        }

        self.0 |= bits << DUMPING_SHIFT;
    }

    pub(crate) fn set_telemetry_per_actor_group_enabled(&mut self, is_enabled: bool) {
        if is_enabled {
            self.0 |= TELEMETRY_PER_ACTOR_GROUP_IS_ENABLED;
        } else {
            self.0 &= !TELEMETRY_PER_ACTOR_GROUP_IS_ENABLED;
        }
    }

    pub(crate) fn set_telemetry_per_actor_key_enabled(&mut self, is_enabled: bool) {
        if is_enabled {
            self.0 |= TELEMETRY_PER_ACTOR_KEY_IS_ENABLED;
        } else {
            self.0 &= !TELEMETRY_PER_ACTOR_KEY_IS_ENABLED;
        }
    }
}

fn log_level_to_value(level: tracing::Level) -> u32 {
    use tracing::Level;

    // The compiler should optimize this expression because `tracing::Level` is
    // based on the same values internally.
    match level {
        Level::TRACE => 0,
        Level::DEBUG => 1,
        Level::INFO => 2,
        Level::WARN => 3,
        Level::ERROR => 4,
    }
}

fn dump_level_filter_to_value(level: Option<dumping::Level>) -> u64 {
    let level = level.map_or(0, |level| level as u64);
    assert!(level <= DUMPING_CLASS_MASK);
    level
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logging_flags_work() {
        use tracing::Level;

        let mut perm = Permissions::default();

        assert!(!perm.is_logging_enabled(Level::ERROR));
        assert!(!perm.is_logging_enabled(Level::TRACE));

        perm.set_logging_enabled(Some(Level::INFO));
        assert!(perm.is_logging_enabled(Level::ERROR));
        assert!(perm.is_logging_enabled(Level::WARN));
        assert!(perm.is_logging_enabled(Level::INFO));
        assert!(!perm.is_logging_enabled(Level::DEBUG));
        assert!(!perm.is_logging_enabled(Level::TRACE));

        perm.set_logging_enabled(Some(Level::ERROR));
        assert!(perm.is_logging_enabled(Level::ERROR));
        assert!(!perm.is_logging_enabled(Level::WARN));
        assert!(!perm.is_logging_enabled(Level::INFO));
        assert!(!perm.is_logging_enabled(Level::DEBUG));
        assert!(!perm.is_logging_enabled(Level::TRACE));

        perm.set_logging_enabled(Some(Level::TRACE));
        assert!(perm.is_logging_enabled(Level::ERROR));
        assert!(perm.is_logging_enabled(Level::WARN));
        assert!(perm.is_logging_enabled(Level::INFO));
        assert!(perm.is_logging_enabled(Level::DEBUG));
        assert!(perm.is_logging_enabled(Level::TRACE));

        perm.set_logging_enabled(None);
        assert!(!perm.is_logging_enabled(Level::ERROR));
        assert!(!perm.is_logging_enabled(Level::TRACE));
    }

    #[test]
    fn dumping_flags_work() {
        use dumping::Level;

        let mut perm = Permissions::default();

        assert!(!perm.is_dumping_enabled(0, Level::Normal));
        assert!(!perm.is_dumping_enabled(10, Level::Normal));

        // Only default.
        // Order of groups are shuffled to check that the order doesn't matter.

        perm.set_dumping_enabled(None, []);
        assert!(!perm.is_dumping_enabled(DUMPING_CLASSES as u8, Level::Normal));

        for class_no in 0..(DUMPING_CLASSES as u8) {
            assert!(!perm.is_dumping_enabled(class_no, Level::Normal));
            assert!(!perm.is_dumping_enabled(class_no, Level::Verbose));
            assert!(!perm.is_dumping_enabled(class_no, Level::Total));
            assert!(!perm.is_dumping_enabled(class_no, Level::Never));
        }

        perm.set_dumping_enabled(Some(Level::Total), []);
        assert!(!perm.is_dumping_enabled(DUMPING_CLASSES as u8, Level::Normal));

        for class_no in 0..(DUMPING_CLASSES as u8) {
            assert!(perm.is_dumping_enabled(class_no, Level::Normal));
            assert!(perm.is_dumping_enabled(class_no, Level::Verbose));
            assert!(perm.is_dumping_enabled(class_no, Level::Total));
            assert!(!perm.is_dumping_enabled(class_no, Level::Never));
        }

        perm.set_dumping_enabled(Some(Level::Normal), []);
        assert!(!perm.is_dumping_enabled(DUMPING_CLASSES as u8, Level::Normal));

        for class_no in 0..(DUMPING_CLASSES as u8) {
            assert!(perm.is_dumping_enabled(class_no, Level::Normal));
            assert!(!perm.is_dumping_enabled(class_no, Level::Verbose));
            assert!(!perm.is_dumping_enabled(class_no, Level::Total));
            assert!(!perm.is_dumping_enabled(class_no, Level::Never));
        }

        perm.set_dumping_enabled(Some(Level::Verbose), []);
        assert!(!perm.is_dumping_enabled(DUMPING_CLASSES as u8, Level::Normal));

        for class_no in 0..(DUMPING_CLASSES as u8) {
            assert!(perm.is_dumping_enabled(class_no, Level::Normal));
            assert!(perm.is_dumping_enabled(class_no, Level::Verbose));
            assert!(!perm.is_dumping_enabled(class_no, Level::Total));
            assert!(!perm.is_dumping_enabled(class_no, Level::Never));
        }

        // Default + overrides.

        let overrides = vec![
            (0, Some(Level::Total)),
            (7, Some(Level::Verbose)),
            (8, None),
            ((DUMPING_CLASSES - 1) as u8, Some(Level::Verbose)),
            (DUMPING_CLASSES as u8, Some(Level::Total)), // should be ignored
        ];

        perm.set_dumping_enabled(Some(Level::Normal), overrides.iter().cloned());
        assert!(!perm.is_dumping_enabled(DUMPING_CLASSES as u8, Level::Normal));

        for class_no in 0..(DUMPING_CLASSES as u8) {
            if overrides.iter().any(|(no, _)| *no == class_no) {
                continue;
            }

            assert!(perm.is_dumping_enabled(class_no, Level::Normal));
            assert!(!perm.is_dumping_enabled(class_no, Level::Verbose));
            assert!(!perm.is_dumping_enabled(class_no, Level::Total));
            assert!(!perm.is_dumping_enabled(class_no, Level::Never));
        }

        assert!(perm.is_dumping_enabled(0, Level::Normal));
        assert!(perm.is_dumping_enabled(0, Level::Verbose));
        assert!(perm.is_dumping_enabled(0, Level::Total));
        assert!(perm.is_dumping_enabled(7, Level::Normal));
        assert!(perm.is_dumping_enabled(7, Level::Verbose));
        assert!(!perm.is_dumping_enabled(7, Level::Total));
        assert!(!perm.is_dumping_enabled(8, Level::Normal));
        assert!(!perm.is_dumping_enabled(8, Level::Verbose));
        assert!(!perm.is_dumping_enabled(8, Level::Total));
        assert!(perm.is_dumping_enabled((DUMPING_CLASSES - 1) as u8, Level::Normal));
        assert!(perm.is_dumping_enabled((DUMPING_CLASSES - 1) as u8, Level::Verbose));
        assert!(!perm.is_dumping_enabled((DUMPING_CLASSES - 1) as u8, Level::Total));
    }

    #[test]
    fn telemetry_flags_work() {
        let mut perm = Permissions::default();

        assert!(!perm.is_telemetry_per_actor_group_enabled());
        assert!(!perm.is_telemetry_per_actor_key_enabled());

        perm.set_telemetry_per_actor_key_enabled(true);
        assert!(!perm.is_telemetry_per_actor_group_enabled());
        assert!(perm.is_telemetry_per_actor_key_enabled());

        perm.set_telemetry_per_actor_group_enabled(true);
        assert!(perm.is_telemetry_per_actor_group_enabled());
        assert!(perm.is_telemetry_per_actor_key_enabled());

        perm.set_telemetry_per_actor_key_enabled(false);
        assert!(perm.is_telemetry_per_actor_group_enabled());
        assert!(!perm.is_telemetry_per_actor_key_enabled());

        perm.set_telemetry_per_actor_group_enabled(false);
        assert!(!perm.is_telemetry_per_actor_group_enabled());
        assert!(!perm.is_telemetry_per_actor_key_enabled());
    }
}
