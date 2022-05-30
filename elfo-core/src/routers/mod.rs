use std::{
    fmt::{self, Display},
    hash::Hash,
};

use elfo_macros::msg_raw as msg;

use crate::{self as elfo, envelope::Envelope};

pub use self::map::MapRouter;

mod map;

pub trait Router<C>: Send + Sync + 'static {
    type Key: Clone + Hash + Eq + Display + Send + Sync; // TODO: why is `Sync` required?

    fn update(&self, _config: &C) {}
    fn route(&self, envelope: &Envelope) -> Outcome<Self::Key>;
}

/// Specifies which actors will get a message.
#[derive(Debug)] // TODO(v0.2): mark as `non_exhaustive`.
pub enum Outcome<T> {
    /// Routes a message to an actor with the specified key.
    /// If there is no active or restarting actor under this key,
    /// such a actor will be started.
    Unicast(T),
    /// Routes a message to all actors with specified keys.
    /// If there are no active or restarting actors under these keys,
    /// corresponding actors will be started.
    Multicast(Vec<T>), // TODO(v0.2): use `SmallVec`?
    /// Routes a message to all active actors.
    Broadcast,
    /// Discards a message.
    /// If a message is discarded by everyone, the sending side gets an error.
    Discard,
    /// Acts as `Broadcast` for system messages and `Discard` otherwise.
    Default,
}

assert_eq_size!(Outcome<u64>, [u8; 32]);
assert_eq_size!(Outcome<u128>, [u8; 32]);

impl<T> Outcome<T> {
    /// Transforms `Unicast` and `Multicast` variants.
    #[inline]
    pub fn map<U>(self, mut f: impl FnMut(T) -> U) -> Outcome<U> {
        match self {
            Outcome::Unicast(val) => Outcome::Unicast(f(val)),
            Outcome::Multicast(list) => Outcome::Multicast(list.into_iter().map(f).collect()),
            Outcome::Broadcast => Outcome::Broadcast,
            Outcome::Discard => Outcome::Discard,
            Outcome::Default => Outcome::Default,
        }
    }

    /// Replaces the `Default` variant with the provided one.
    #[inline]
    pub fn or(self, outcome: Outcome<T>) -> Self {
        match self {
            Outcome::Default => outcome,
            _ => self,
        }
    }
}

impl<C> Router<C> for () {
    type Key = Singleton;

    #[inline]
    fn route(&self, envelope: &Envelope) -> Outcome<Self::Key> {
        use crate::messages::*;

        msg!(match envelope {
            // These messages shouldn't spawn actors.
            // TODO: maybe this logic should be in the supervisor.
            ValidateConfig | Terminate | Ping => Outcome::Broadcast,
            _ => Outcome::Unicast(Singleton),
        })
    }
}

/// A key used for actor groups containing only one actor.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct Singleton;

impl fmt::Display for Singleton {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // If this line is changed, change also a key in `start.rs` to be consistent.
        f.write_str("_")
    }
}
