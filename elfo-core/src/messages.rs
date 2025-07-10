use std::{fmt::Display, sync::Arc};

use derive_more::Constructor;

use crate::{
    actor::ActorMeta, actor_status::ActorStatus, config::AnyConfig, message, signal::SignalKind,
};

/// A helper type for using in generic code (e.g. as an associated type) to
/// indicate a message that cannot be constructed.
///
/// Feel free to define a custom `Impossible` type as an empty enum if more
/// implemented traits are needed.
#[message]
#[derive(Eq, PartialEq, Hash, Copy)]
pub enum Impossible {}

/// Checks that the actor is able to handle messages.
/// Routed to all actors in a group by default and handled implicitly by actors.
#[message(ret = ())]
#[derive(Default)]
#[non_exhaustive]
pub struct Ping;

#[message(ret = Result<(), ConfigRejected>)]
#[derive(Constructor)]
#[non_exhaustive]
pub struct ValidateConfig {
    pub config: AnyConfig,
}

#[message(ret = Result<(), ConfigRejected>)]
#[derive(Constructor)]
#[non_exhaustive]
pub struct UpdateConfig {
    pub config: AnyConfig,
}

#[message]
#[non_exhaustive]
pub struct ConfigRejected {
    pub reason: String,
}

impl<R: Display> From<R> for ConfigRejected {
    fn from(reason: R) -> Self {
        Self {
            reason: reason.to_string(),
        }
    }
}

#[message(ret = Result<(), StartEntrypointRejected>)]
#[derive(Constructor)]
#[non_exhaustive]
pub struct StartEntrypoint {
    pub is_check_only: bool,
}

#[message(part)]
#[derive(Constructor)]
#[non_exhaustive]
pub struct StartEntrypointRejected {
    pub errors: Vec<EntrypointError>,
}

#[message(part)]
#[derive(Constructor)]
#[non_exhaustive]
pub struct EntrypointError {
    pub group: String,
    pub reason: String,
}

#[message]
#[non_exhaustive]
pub struct ConfigUpdated {
    // TODO: add `old_config`.
}

#[message]
#[derive(Default)]
#[non_exhaustive]
pub struct Terminate {
    pub(crate) closing: bool,
    pub reason: Option<TerminateReason>,
}

impl Terminate {
    /// The message closes a target's mailbox ignoring `TerminationPolicy`.
    pub fn closing() -> Self {
        Self {
            closing: true,
            reason: None,
        }
    }

    /// Create terminate message with provided reason
    pub fn with_reason(reason: impl Into<Option<TerminateReason>>) -> Self {
        Self {
            closing: false,
            reason: reason.into(),
        }
    }
}

#[message(part)]
#[derive(Default, PartialEq)]
#[non_exhaustive]
pub enum TerminateReason {
    #[default]
    Unknown,
    Signal(SignalKind),
    Oom,
    Custom(Arc<str>),
}

impl TerminateReason {
    pub fn custom(reason: impl Into<Arc<str>>) -> Self {
        Self::Custom(reason.into())
    }
}

// === Status ===

// TODO: should it be a request?
#[message]
#[derive(Default)]
#[non_exhaustive]
pub struct SubscribeToActorStatuses {
    pub(crate) forcing: bool,
}

impl SubscribeToActorStatuses {
    /// Statuses will be sent even if a subscriber is already registered.
    pub fn forcing() -> Self {
        Self { forcing: true }
    }
}

#[message]
#[non_exhaustive]
pub struct ActorStatusReport {
    pub meta: Arc<ActorMeta>,
    pub status: ActorStatus,
}

impl ActorStatusReport {
    #[cfg(feature = "test-util")]
    pub fn new(meta: impl Into<Arc<ActorMeta>>, status: ActorStatus) -> Self {
        Self {
            meta: meta.into(),
            status,
        }
    }
}
