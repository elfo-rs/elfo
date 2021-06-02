use std::sync::Arc;

use sharded_slab::{self as slab, Slab};

use crate::{
    addr::Addr,
    object::{Object, ObjectArc, ObjectRef},
};

#[derive(Clone)]
pub(crate) struct AddressBook {
    slab: Arc<Slab<Object>>,
}

assert_impl_all!(AddressBook: Sync);

impl AddressBook {
    pub(crate) fn new() -> Self {
        Self {
            slab: Arc::new(Slab::new()),
        }
    }

    pub(crate) fn get(&self, addr: Addr) -> Option<ObjectRef<'_>> {
        self.slab.get(addr.into_bits())
    }

    pub(crate) fn get_owned(&self, addr: Addr) -> Option<ObjectArc> {
        self.slab.clone().get_owned(addr.into_bits())
    }

    pub(crate) fn vacant_entry(&self) -> VacantEntry<'_> {
        self.slab
            .vacant_entry()
            .map(VacantEntry)
            .expect("too many actors")
    }

    pub(crate) fn remove(&self, addr: Addr) {
        self.slab.remove(addr.into_bits());
    }
}

pub(crate) struct VacantEntry<'b>(slab::VacantEntry<'b, Object>);

impl<'b> VacantEntry<'b> {
    pub(crate) fn insert(self, object: Object) {
        self.0.insert(object)
    }

    pub(crate) fn addr(&self) -> Addr {
        Addr::from_bits(self.0.key())
    }
}
