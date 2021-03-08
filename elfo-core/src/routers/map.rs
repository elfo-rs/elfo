use std::hash::Hash;

use super::{Outcome, Router};
use crate::envelope::Envelope;

pub struct MapRouter<F>(F);

impl<F, K> MapRouter<F>
where
    F: Fn(&Envelope) -> Outcome<K>,
    K: Into<u64>,
    K: Clone + Hash + Eq,
{
    #[inline]
    pub fn new(f: F) -> MapRouter<F> {
        Self(f)
    }
}

impl<F, K> Router for MapRouter<F>
where
    F: Fn(&Envelope) -> Outcome<K> + Send + Sync + 'static,
{
    type Key = K;

    #[inline]
    fn route(&self, envelope: &Envelope) -> Outcome<Self::Key> {
        (self.0)(envelope)
    }
}
