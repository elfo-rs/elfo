use std::{
    fmt, mem,
    sync::atomic::{self, AtomicU8},
};

use serde::{Deserialize, Serialize};

// === ActorStatus ===

/// Represents the current status of an actor.
/// See [The Actoromicon](https://actoromicon.rs/ch03-01-actor-lifecycle.html) for details.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActorStatus {
    pub(crate) kind: ActorStatusKind,
    pub(crate) details: Option<String>,
}

impl ActorStatus {
    const fn new(kind: ActorStatusKind) -> Self {
        Self {
            kind,
            details: None,
        }
    }

    /// Creates a new status with the same kind and provided details.
    pub fn with_details(&self, details: impl fmt::Display) -> Self {
        ActorStatus {
            kind: self.kind,
            details: Some(details.to_string()),
        }
    }

    /// Returns the corresponding [`ActorStatusKind`] for this status.
    pub fn kind(&self) -> ActorStatusKind {
        self.kind
    }

    /// Returns details for this status, if provided.
    pub fn details(&self) -> Option<&str> {
        self.details.as_deref()
    }
}

#[cfg(not(feature = "test-util"))]
impl ActorStatus {
    pub const ALARMING: ActorStatus = ActorStatus::new(ActorStatusKind::Alarming);
    pub(crate) const FAILED: ActorStatus = ActorStatus::new(ActorStatusKind::Failed);
    pub const INITIALIZING: ActorStatus = ActorStatus::new(ActorStatusKind::Initializing);
    pub const NORMAL: ActorStatus = ActorStatus::new(ActorStatusKind::Normal);
    pub(crate) const TERMINATED: ActorStatus = ActorStatus::new(ActorStatusKind::Terminated);
    pub const TERMINATING: ActorStatus = ActorStatus::new(ActorStatusKind::Terminating);
}

#[cfg(feature = "test-util")]
impl ActorStatus {
    pub const ALARMING: ActorStatus = ActorStatus::new(ActorStatusKind::Alarming);
    pub const FAILED: ActorStatus = ActorStatus::new(ActorStatusKind::Failed);
    pub const INITIALIZING: ActorStatus = ActorStatus::new(ActorStatusKind::Initializing);
    pub const NORMAL: ActorStatus = ActorStatus::new(ActorStatusKind::Normal);
    pub const TERMINATED: ActorStatus = ActorStatus::new(ActorStatusKind::Terminated);
    pub const TERMINATING: ActorStatus = ActorStatus::new(ActorStatusKind::Terminating);
}

impl fmt::Display for ActorStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.details {
            Some(details) => write!(f, "{:?}: {}", self.kind, details),
            None => write!(f, "{:?}", self.kind),
        }
    }
}

// === ActorStatusKind ===

/// A list specifying statuses of actors. It's used with the [`ActorStatus`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
#[repr(u8)]
pub enum ActorStatusKind {
    Normal,
    Initializing,
    Terminating,
    Terminated,
    Alarming,
    Failed,
}

impl ActorStatusKind {
    #[inline]
    pub const fn is_normal(&self) -> bool {
        matches!(self, Self::Normal)
    }

    #[inline]
    pub const fn is_initializing(&self) -> bool {
        matches!(self, Self::Initializing)
    }

    #[inline]
    pub const fn is_terminating(&self) -> bool {
        matches!(self, Self::Terminating)
    }

    #[inline]
    pub const fn is_terminated(&self) -> bool {
        matches!(self, Self::Terminated)
    }

    #[inline]
    pub const fn is_alarming(&self) -> bool {
        matches!(self, Self::Alarming)
    }

    #[inline]
    pub const fn is_failed(&self) -> bool {
        matches!(self, Self::Failed)
    }

    #[inline]
    pub const fn is_finished(&self) -> bool {
        self.is_failed() || self.is_terminated()
    }
}

impl ActorStatusKind {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            ActorStatusKind::Normal => "Normal",
            ActorStatusKind::Initializing => "Initializing",
            ActorStatusKind::Terminating => "Terminating",
            ActorStatusKind::Terminated => "Terminated",
            ActorStatusKind::Alarming => "Alarming",
            ActorStatusKind::Failed => "Failed",
        }
    }
}

// === AtomicActorStatusKind ===

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
        // place where value may be changed is `Self::store`, which consumes
        // `ActorStatusKind`, thus, guarantees that possibly invalid value
        // cannot be stored
        unsafe { mem::transmute::<u8, ActorStatusKind>(result) }
    }
}
