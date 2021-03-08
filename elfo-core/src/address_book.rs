use std::sync::Arc;

use sharded_slab::Slab;

use crate::{
    addr::Addr,
    object::{Object, ObjectArc, ObjectRef},
};

#[derive(Clone)]
pub(crate) struct AddressBook {
    // TODO: remove an extra reference.
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

    pub(crate) fn insert_with_addr(&self, f: impl FnOnce(Addr) -> Object) -> Addr {
        let entry = self.slab.vacant_entry().expect("too many actors");
        let key = Addr::from_bits(entry.key());
        entry.insert(f(key));
        key
    }
}
