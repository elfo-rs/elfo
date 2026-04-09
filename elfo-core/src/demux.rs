use std::sync::Arc;

use smallvec::SmallVec;

use crate::{Addr, envelope::Envelope};

const OPTIMAL_COUNT: usize = 5;
type Addrs = SmallVec<[Addr; OPTIMAL_COUNT]>;

type Filter = Arc<dyn Fn(&Envelope, &mut Addrs) + Send + Sync>;

// Actually, it's a private type, `pub` is for `Destination` only.
#[derive(Default, Clone)]
pub struct Demux {
    filters: SmallVec<[Filter; OPTIMAL_COUNT]>,
}

impl Demux {
    pub(crate) fn append(&mut self, f: impl Fn(&Envelope, &mut Addrs) + Send + Sync + 'static) {
        self.filters.push(Arc::new(f));
    }

    // TODO: return an iterator?
    pub(crate) fn filter(&self, envelope: &Envelope) -> Addrs {
        let mut addrs = Addrs::new();
        for f in &self.filters {
            f(envelope, &mut addrs);
        }
        addrs
    }
}
