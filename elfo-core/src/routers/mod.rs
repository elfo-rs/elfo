use std::{fmt::Display, hash::Hash};

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
    // TODO
    type Key = u32;

    #[inline]
    fn route(&self, _: &Envelope) -> Outcome<Self::Key> {
        Outcome::Unicast(0)
    }
}
