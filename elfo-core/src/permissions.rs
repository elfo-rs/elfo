use std::sync::atomic::{AtomicUsize, Ordering};

// Layout:
//
//      7 6   4 3 2 1 0
//     +-+-+-+-+-+-+-+-+
//     |G|K| |E|W|I|D|T|
//     +-+-+-+-+-+-+-+-+
//      | |  '---------'
//      | |    |
//      | |    +- logging levels
//      | +------ telemetry per actor key
//      +-------- telemetry per actor group
//
// Reexported in `elfo::_priv`.
#[derive(Default)]
pub struct AtomicPermissions(AtomicUsize);

const LOGGING_MASK: usize = 0b00011111;
const TELEMETRY_PER_ACTOR_GROUP_IS_ENABLED: usize = 0b01000000;
const TELEMETRY_PER_ACTOR_KEY_IS_ENABLED: usize = 0b10000000;

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
pub struct Permissions(usize);

impl Permissions {
    #[inline]
    pub fn is_logging_enabled(&self, level: tracing::Level) -> bool {
        let mask = 1 << log_level_to_value(level);
        self.0 & mask != 0
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
