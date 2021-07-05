use std::{
    fmt::{self, Display},
    hash::Hash,
};

use crate::envelope::Envelope;

pub use self::map::MapRouter;

mod map;

pub trait Router<C>: Send + Sync + 'static {
    type Key: Clone + Hash + Eq + Display + Send + Sync; // TODO: why is `Sync` required?

    fn update(&self, _config: &C) {}
    fn route(&self, envelope: &Envelope) -> Outcome<Self::Key>;
}

#[derive(Debug)]
pub enum Outcome<T> {
    Unicast(T),
    Multicast(Vec<T>), // TODO: use `SmallVec`?
    Broadcast,
    Discard,
    Default,
}

assert_eq_size!(Outcome<u64>, [u8; 32]);
assert_eq_size!(Outcome<u128>, [u8; 32]);

impl<T> Outcome<T> {
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
    fn route(&self, _: &Envelope) -> Outcome<Self::Key> {
        Outcome::Unicast(Singleton)
    }
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct Singleton;

impl fmt::Display for Singleton {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // If this line is changed, change also a key in `start.rs` to be consistent.
        f.write_str("_")
    }
}
