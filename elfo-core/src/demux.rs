use std::sync::Arc;

use smallvec::SmallVec;

use crate::{envelope::Envelope, Addr};

const OPTIMAL_COUNT: usize = 5;

#[derive(Default, Clone)]
pub(crate) struct Demux {
    list: SmallVec<[(Addr, Filter); OPTIMAL_COUNT]>,
}

impl Demux {
    pub(crate) fn append(&mut self, addr: Addr, f: Filter) {
        self.list.push((addr, f));
    }

    pub(crate) fn filter(&self, envelope: &Envelope) -> SmallVec<[Addr; OPTIMAL_COUNT]> {
        // TODO: use a bitset as iterator's state.
        self.list
            .iter()
            .filter(|(_, filter)| match filter {
                Filter::Dynamic(filter) => filter(envelope),
            })
            .map(|(addr, _)| *addr)
            .collect()
    }
}

#[derive(Clone)]
pub(crate) enum Filter {
    // TODO: what about `smallbox`?
    Dynamic(Arc<dyn Fn(&Envelope) -> bool + Send + Sync>),
}
