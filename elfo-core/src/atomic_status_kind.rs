use std::mem;
use std::sync::atomic::{self, AtomicU8};

use crate::ActorStatusKind;

#[derive(Debug)]
#[repr(transparent)]
pub(crate) struct AtomicActorStatusKind(AtomicU8);

impl From<ActorStatusKind> for AtomicActorStatusKind {
    fn from(value: ActorStatusKind) -> Self {
        Self(AtomicU8::new(value as _))
    }
}

impl AtomicActorStatusKind {
    pub(crate) fn store(&self, kind: ActorStatusKind, ordering: atomic::Ordering) {
        self.0.store(kind as u8, ordering);
    }

    pub(crate) fn load(&self, ordering: atomic::Ordering) -> ActorStatusKind {
        let result = self.0.load(ordering);

        // SAFETY: `ActorStatusKind` has `#[repr(u8)]` annotation. The only
        // place where value may be changed is `Self::store`, which consumes `ActorStatusKind`, thus,
        // guarantees that possibly invalid value cannot be stored
        unsafe { mem::transmute::<u8, ActorStatusKind>(result) }
    }
}
