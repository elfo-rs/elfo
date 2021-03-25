use std::{marker::PhantomData, sync::Arc};

use arc_swap::ArcSwap;

use super::{Outcome, Router};
use crate::envelope::Envelope;

pub struct MapRouter<C, S, P, R> {
    config: PhantomData<C>,
    state: S,
    prepare: P,
    route: R,
}

// Just router.
impl<C, R, K> MapRouter<C, (), (), R>
where
    C: Send + Sync + 'static,
    R: Fn(&Envelope) -> Outcome<K> + Send + Sync + 'static,
{
    #[inline]
    pub fn new(route: R) -> Self {
        Self {
            config: PhantomData,
            state: (),
            prepare: (),
            route,
        }
    }
}

impl<C, R, K> Router<C> for MapRouter<C, (), (), R>
where
    C: Send + Sync + 'static,
    R: Fn(&Envelope) -> Outcome<K> + Send + Sync + 'static,
{
    type Key = K;

    #[inline]
    fn route(&self, envelope: &Envelope) -> Outcome<Self::Key> {
        (self.route)(envelope)
    }
}

// With state.
impl<C, S, P, R, K> MapRouter<C, ArcSwap<S>, P, R>
where
    C: Send + Sync + 'static,
    S: Default + Send + Sync + 'static,
    R: Fn(&Envelope, &S) -> Outcome<K> + Send + Sync + 'static,
    P: Fn(&C, &S) -> S + Send + Sync + 'static,
{
    #[inline]
    pub fn with_state(prepare: P, route: R) -> Self {
        Self {
            config: PhantomData,
            state: ArcSwap::default(),
            prepare,
            route,
        }
    }
}

impl<C, S, P, R, K> Router<C> for MapRouter<C, ArcSwap<S>, P, R>
where
    C: Send + Sync + 'static,
    S: Send + Sync + 'static,
    R: Fn(&Envelope, &S) -> Outcome<K> + Send + Sync + 'static,
    P: Fn(&C, &S) -> S + Send + Sync + 'static,
{
    type Key = K;

    #[inline]
    fn update(&self, config: &C) {
        self.state
            .rcu(|state| Arc::new((self.prepare)(config, state)));
    }

    #[inline]
    fn route(&self, envelope: &Envelope) -> Outcome<Self::Key> {
        let state = self.state.load();
        (self.route)(envelope, &state)
    }
}
