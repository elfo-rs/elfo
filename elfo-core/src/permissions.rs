use std::sync::atomic::{AtomicUsize, Ordering};

// Reexported in `elfo::_priv`.
#[derive(Default)]
pub struct AtomicPermissions(AtomicUsize);

const TELEMETRY_PER_ACTOR_GROUP_IS_ENABLED: usize = 1 << 31;
const TELEMETRY_PER_ACTOR_KEY_IS_ENABLED: usize = 1 << 30;

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
#[derive(Debug, Clone, Copy)]
pub struct Permissions(usize);

impl Permissions {
    #[inline]
    pub fn is_telemetry_per_actor_group_enabled(&self) -> bool {
        self.0 & TELEMETRY_PER_ACTOR_GROUP_IS_ENABLED != 0
    }

    #[inline]
    pub fn is_telemetry_per_actor_key_enabled(&self) -> bool {
        self.0 & TELEMETRY_PER_ACTOR_KEY_IS_ENABLED != 0
    }

    pub fn set_telemetry_per_actor_group_enabled(&mut self, is_enabled: bool) {
        if is_enabled {
            self.0 |= TELEMETRY_PER_ACTOR_GROUP_IS_ENABLED;
        } else {
            self.0 &= !TELEMETRY_PER_ACTOR_GROUP_IS_ENABLED;
        }
    }

    pub fn set_telemetry_per_actor_key_enabled(&mut self, is_enabled: bool) {
        if is_enabled {
            self.0 |= TELEMETRY_PER_ACTOR_KEY_IS_ENABLED;
        } else {
            self.0 &= !TELEMETRY_PER_ACTOR_KEY_IS_ENABLED;
        }
    }
}
