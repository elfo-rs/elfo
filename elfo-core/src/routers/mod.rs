use crate::envelope::Envelope;

pub use self::map::MapRouter;

mod map;

pub trait Router: Send + Sync + 'static {
    type Key;

    // TODO: pass `ctx` or `config`.
    fn route(&self, envelope: &Envelope) -> Outcome<Self::Key>;
}

#[derive(Debug)]
pub enum Outcome<T> {
    Unicast(T),
    Broadcast,
    Discard,
}

impl<T> Outcome<T> {
    #[inline]
    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> Outcome<U> {
        match self {
            Outcome::Unicast(val) => Outcome::Unicast(f(val)),
            Outcome::Broadcast => Outcome::Broadcast,
            Outcome::Discard => Outcome::Discard,
        }
    }
}

impl Router for () {
    type Key = u32;

    #[inline]
    fn route(&self, _: &Envelope) -> Outcome<Self::Key> {
        Outcome::Unicast(0)
    }
}
