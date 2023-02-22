use std::{fmt::Display, sync::Arc};

use derive_more::Constructor;

use crate::{
    actor::{ActorMeta, ActorStatus},
    config::AnyConfig,
    message,
};

/// Checks that the actor is able to handle messages.
/// Routed to all actors in a group by default and handled implicitly by actors.
#[message(ret = ())]
pub struct Ping;

#[message(ret = Result<(), ConfigRejected>)]
#[derive(Constructor)]
pub struct ValidateConfig {
    pub config: AnyConfig,
}

#[message(ret = Result<(), ConfigRejected>)]
#[derive(Constructor)]
pub struct UpdateConfig {
    pub config: AnyConfig,
}

#[message]
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

#[message]
pub struct ConfigUpdated {
    // TODO: add `old_config`.
}

#[message]
#[derive(Default)]
pub struct Terminate {
    pub(crate) closing: bool,
}

impl Terminate {
    /// The message closes a target's mailbox ignoring `TerminationPolicy`.
    pub fn closing() -> Self {
        Self { closing: true }
    }
}

// === Status ===

// TODO: should it be a request?
#[message]
#[derive(Default)]
#[non_exhaustive]
pub struct SubscribeToActorStatuses {}

#[message]
#[non_exhaustive]
pub struct ActorStatusReport {
    pub meta: Arc<ActorMeta>,
    pub status: ActorStatus,
}
