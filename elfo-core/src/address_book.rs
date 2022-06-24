use std::{fmt, sync::Arc};

use sharded_slab::{self as slab, Slab};
use static_assertions::const_assert_eq;

use crate::{
    node::{self, NodeNo},
    object::{Object, ObjectArc, ObjectRef},
};

#[stability::unstable]
pub type GroupNo = u8;

// Structure (64b platform):
//        16b         8b         10b         9b         21b
//  +------------+----------+------------+-------+-----------------+
//  |   node_no  | group_no | generation |  TID  |  page + offset  |
//  +------------+----------+------------+-------+-----------------+
//   (0 if local)           ^----------- slot addr (40b) ----------^
//
// Limits:
// - max active actors spawned by one thread    1048544
// - slot generations to prevent ABA               1024
// - max threads spawning actors                    256
// - max groups                                     255 *
//
// Structure (32b platform):
//        16b         8b       8b       7b      7b       18b
//  +------------+----------+-------+--------+-----+---------------+
//  |   node_no  | group_no | empty | genera | TID | page + offset |
//  +------------+----------+-------+--------+-----+---------------+
//   (0 if local)                   ^------- slot addr (32b) ------^
//
// Limits:
// - max active actors spawned by one thread     131040
// - slot generations to prevent ABA                128
// - max threads spawning actors                     64
// - max groups                                     255 *
//
// * - `GroupNo::MAX` is reserved for `NULL`.
//
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Addr(u64);

impl fmt::Display for Addr {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let node_no = self.node_no();
        let group_no = self.group_no();
        let slot_addr = self.slot_addr();

        if node_no == node::LOCAL_NODE_NO {
            write!(f, "{}:{}", group_no, slot_addr)
        } else {
            write!(f, "{}:{}:{}", node_no, group_no, slot_addr)
        }
    }
}

impl Addr {
    pub const NULL: Addr = Addr(0x0000_ffff_ffff_ffff);

    fn new_local(slot_addr: usize, group_no: GroupNo) -> Self {
        Addr(u64::from(group_no) << 40 | slot_addr as u64)
    }

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
    pub fn group_no(self) -> GroupNo {
        (self.0 >> 40) as GroupNo
    }

    fn slot_addr(self) -> usize {
        (self.0 & 0x0000_00ff_ffff_ffff) as usize
    }

    #[stability::unstable]
    #[inline]
    pub fn into_remote(self) -> Self {
        if self != Self::NULL && self.node_no() == node::LOCAL_NODE_NO {
            Self(self.0 | u64::from(node::node_no()) << 48)
        } else {
            self
        }
    }

    #[stability::unstable]
    #[inline]
    pub fn into_local(self) -> Self {
        Self(self.0 & 0x0000_ffff_ffff_ffff)
    }
}

pub(crate) struct SlabConfig;

#[cfg(target_pointer_width = "64")]
impl sharded_slab::Config for SlabConfig {
    const INITIAL_PAGE_SIZE: usize = 32;
    const MAX_PAGES: usize = 15;
    const MAX_THREADS: usize = 256;
    const RESERVED_BITS: usize = 24;
}
#[cfg(target_pointer_width = "64")]
const_assert_eq!(Slab::<Object, SlabConfig>::USED_BITS, 40);

#[cfg(target_pointer_width = "32")]
impl sharded_slab::Config for SlabConfig {
    const INITIAL_PAGE_SIZE: usize = 32;
    const MAX_PAGES: usize = 12;
    const MAX_THREADS: usize = 64;
    const RESERVED_BITS: usize = 0;
}
#[cfg(target_pointer_width = "32")]
const_assert_eq!(Slab::<Object, SlabConfig>::USED_BITS, 32);

#[derive(Clone)]
pub(crate) struct AddressBook {
    slab: Arc<Slab<Object, SlabConfig>>,
}

assert_impl_all!(AddressBook: Sync);

impl AddressBook {
    pub(crate) fn new() -> Self {
        Self {
            slab: Arc::new(Slab::new_with_config::<SlabConfig>()),
        }
    }

    pub(crate) fn get(&self, addr: Addr) -> Option<ObjectRef<'_>> {
        self.slab.get(addr.into_bits() as usize)
    }

    pub(crate) fn get_owned(&self, addr: Addr) -> Option<ObjectArc> {
        self.slab.clone().get_owned(addr.into_bits() as usize)
    }

    pub(crate) fn vacant_entry(&self, group_no: GroupNo) -> VacantEntry<'_> {
        self.slab
            .vacant_entry()
            .map(|entry| VacantEntry { entry, group_no })
            .expect("too many actors")
    }

    pub(crate) fn remove(&self, addr: Addr) {
        self.slab.remove(addr.into_bits() as usize);
    }
}

pub(crate) struct VacantEntry<'b> {
    entry: slab::VacantEntry<'b, Object, SlabConfig>,
    group_no: GroupNo,
}

impl<'b> VacantEntry<'b> {
    pub(crate) fn insert(self, object: Object) {
        self.entry.insert(object)
    }

    pub(crate) fn addr(&self) -> Addr {
        Addr::new_local(self.entry.key(), self.group_no)
    }
}
