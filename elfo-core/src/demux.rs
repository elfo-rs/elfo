use std::sync::Arc;

use smallvec::SmallVec;

use crate::{envelope::Envelope, Addr};

const OPTIMAL_COUNT: usize = 5;
type Addrs = SmallVec<[Addr; OPTIMAL_COUNT]>;

// Actually, it's a private type, `pub` is for `Destination` only.
#[derive(Default, Clone)]
pub struct Demux {
    #[allow(clippy::type_complexity)]
    filter: Option<Arc<dyn Fn(&Envelope, &mut Addrs) + Send + Sync>>,
}

impl Demux {
    pub(crate) fn append(&mut self, f: impl Fn(&Envelope, &mut Addrs) + Send + Sync + 'static) {
        self.filter = Some(if let Some(prev) = self.filter.take() {
            Arc::new(move |envelope, addrs| {
                prev(envelope, addrs);
                f(envelope, addrs);
            })
        } else {
            Arc::new(f)
        })
    }

    // TODO: return an iterator?
    pub(crate) fn filter(&self, envelope: &Envelope) -> Addrs {
        let mut addrs = Addrs::new();
        if let Some(filter) = &self.filter {
            (filter)(envelope, &mut addrs);
        }
        addrs
    }
}
