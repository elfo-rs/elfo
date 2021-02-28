use std::sync::Arc;

use sharded_slab::{self as slab, Slab};

use crate::{addr::Addr, object::Object};

#[derive(Clone)]
pub(crate) struct AddressBook {
    // TODO: remove an extra reference.
    slab: Arc<Slab<Object>>,
}

pub(crate) type Entry<'a> = slab::Entry<'a, Object>;
pub(crate) type OwnedEntry = slab::OwnedEntry<Object>;

impl AddressBook {
    pub(crate) fn new() -> Self {
        Self {
            slab: Arc::new(Slab::new()),
        }
    }

    pub(crate) fn get(&self, addr: Addr) -> Option<Entry<'_>> {
        self.slab.get(addr.into_bits())
    }

    pub(crate) fn get_owned(&self, addr: Addr) -> Option<OwnedEntry> {
        self.slab.clone().get_owned(addr.into_bits())
    }
}
