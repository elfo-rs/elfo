use std::{fmt, sync::Arc};

use sharded_slab::{self as slab, DefaultConfig, Slab};

use crate::{
    node::{self, NodeNo},
    object::{Object, ObjectArc, ObjectRef},
};

// Structure (64b platform):
//  +---------------+-------------------------------------+
//  | node_no (16b) |           local addr (48b)          |
//  |  n if remote  +-------------------------+-----------+
//  |  0 if local   |     generation (35b)    | TID (13b) |
//  +---------------+-------------------------+-----------+
//
// Structure (32b platform):
//  +---------------+-------------+-----------------------+
//  | node_no (16b) | empty (16b) |   local addr (32b)    |
//  |  n if remote  |             +-----------+-----------+
//  |  0 if local   |             | gen (24b) |  TID (8b) |
//  +---------------+-------------+-----------+-----------+
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Addr(u64);

impl fmt::Display for Addr {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}v0", self.0)
    }
}

impl Addr {
    pub const NULL: Addr = Addr(u64::MAX);

    #[stability::unstable]
    #[inline]
    pub fn from_bits(bits: u64) -> Self {
        Addr(bits)
    }

    #[stability::unstable]
    #[inline]
    pub fn into_bits(self) -> u64 {
        self.0
    }

    #[stability::unstable]
    #[inline]
    pub fn is_local(self) -> bool {
        self.node_no() == node::LOCAL_NODE_NO
    }

    #[stability::unstable]
    #[inline]
    pub fn node_no(self) -> NodeNo {
        (self.0 >> 48) as NodeNo
    }

    #[stability::unstable]
    #[inline]
    pub fn into_remote(mut self) -> Self {
        if self.node_no() == node::LOCAL_NODE_NO {
            self.0 |= u64::from(node::node_no()) << 48;
        }

        self
    }

    #[stability::unstable]
    #[inline]
    pub fn into_local(mut self) -> Self {
        self.0 &= !0xffff_0000_0000_0000;
        self
    }
}

#[cfg(target_pointer_width = "64")]
struct SlabConfig;
#[cfg(target_pointer_width = "64")]
impl sharded_slab::Config for SlabConfig {
    const INITIAL_PAGE_SIZE: usize = DefaultConfig::INITIAL_PAGE_SIZE;
    const MAX_PAGES: usize = DefaultConfig::MAX_PAGES;
    const MAX_THREADS: usize = DefaultConfig::MAX_THREADS;
    const RESERVED_BITS: usize = 16;
}

#[cfg(target_pointer_width = "32")]
type SlabConfig = DefaultConfig;

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
        self.slab.get(addr.into_bits() as usize)
    }

    pub(crate) fn get_owned(&self, addr: Addr) -> Option<ObjectArc> {
        self.slab.clone().get_owned(addr.into_bits() as usize)
    }

    pub(crate) fn vacant_entry(&self) -> VacantEntry<'_> {
        self.slab
            .vacant_entry()
            .map(VacantEntry)
            .expect("too many actors")
    }

    pub(crate) fn remove(&self, addr: Addr) {
        self.slab.remove(addr.into_bits() as usize);
    }
}

pub(crate) struct VacantEntry<'b>(slab::VacantEntry<'b, Object>);

impl<'b> VacantEntry<'b> {
    pub(crate) fn insert(self, object: Object) {
        self.0.insert(object)
    }

    pub(crate) fn addr(&self) -> Addr {
        Addr::from_bits(self.0.key() as u64)
    }
}
