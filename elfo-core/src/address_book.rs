use std::{fmt, sync::Arc};

use sharded_slab::{self as slab, Slab};

use crate::{
    node::{self, NodeNo},
    object::{Object, ObjectArc, ObjectRef},
};

#[stability::unstable]
pub type GroupNo = u8;

/// Represents the global unique address of an actor.
///
/// # Using addresses in messages
/// The current implementation of network depends on the fact that
/// `Addr` cannot be sent inside messages. It prevents from different
/// possible errors like responding without having a valid connection.
/// The only way to get an address of remote actor is `envelope.sender()`.
/// If sending `Addr` inside a message is unavoidable, use `Local<Addr>`,
/// however it won't be possible to send such message to a remote actor.
// Structure (64b platform):
//  64           48         40           30      21                0
//  +------------+----------+------------+-------+-----------------+
//  |   node_no  | group_no | generation |  TID  |  page + offset  |
//  |     16b    |    8b    |     10b    |   9b  |       21b       |
//  +------------+----------+------------+-------+-----------------+
//   (0 if local)           ^----------- slot addr (40b) ----------^
//
// Limits:
// - max nodes in a cluster                       65535 (1)
// - max groups in a node                           255 (2)
// - max active actors spawned by one thread    1048544
// - slot generations to prevent ABA               1024
// - max threads spawning actors                    256
//
// Structure (32b platform):
//  64           48         40      32       25    18              0
//  +------------+----------+-------+--------+-----+---------------+
//  |   node_no  | group_no | empty | genera | TID | page + offset |
//  |     16b    |    8b    |   8b  |   7b   |  7b |      18b      |
//  +------------+----------+-------+--------+-----+---------------+
//   (0 if local)                   ^------- slot addr (32b) ------^
//
// Limits:
// - max nodes in a cluster                       65535 (1)
// - max groups in a node                           255 (2)
// - max active actors spawned by one thread     131040
// - slot generations to prevent ABA                128
// - max threads spawning actors                     64
//
// (1) - `0` is reserved to represent the local node.
// (2) - `GroupNo::MAX` is reserved to represent `Addr::NULL` unambiguously.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Addr(u64); // TODO: make it NonZero

// See `Addr` docs for more details.
assert_not_impl_all!(Addr: serde::Serialize, serde::Deserialize<'static>);

impl fmt::Display for Addr {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if *self == Self::NULL {
            return f.write_str("null");
        }

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
    // TODO: make `0`
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

    #[inline]
    pub fn is_null(self) -> bool {
        self == Self::NULL
    }

    #[inline]
    pub fn is_local(self) -> bool {
        !self.is_null() && self.node_no() == node::LOCAL_NODE_NO
    }

    #[inline]
    pub fn is_remote(self) -> bool {
        !self.is_null() && self.node_no() != node::LOCAL_NODE_NO
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

    fn node_no_group_no(self) -> u32 {
        (self.0 >> 40) as u32
    }

    fn slot_addr(self) -> usize {
        (self.0 & 0x0000_00ff_ffff_ffff) as usize
    }

    #[stability::unstable]
    #[inline]
    pub fn into_remote(self) -> Self {
        if self.is_local() {
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

// Actually, it doesn't reexported.
pub struct SlabConfig;

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

// Reexported in `_priv`.
#[derive(Clone)]
pub struct AddressBook {
    local: Arc<Slab<Object, SlabConfig>>,
    #[cfg(feature = "network")]
    remote: Arc<RemoteToHandleMap>, // TODO: use `arc_swap::cache::Cache` in TLS?
}

assert_impl_all!(AddressBook: Sync);

impl AddressBook {
    pub(crate) fn new() -> Self {
        let local = Arc::new(Slab::new_with_config::<SlabConfig>());

        #[cfg(feature = "network")]
        return Self {
            local,
            remote: Default::default(),
        };

        #[cfg(not(feature = "network"))]
        Self { local };
    }

    #[cfg(feature = "network")]
    pub(crate) fn register_remote(
        &self,
        local_group: GroupNo,
        remote_group: (NodeNo, GroupNo),
        handle_addr: Addr,
    ) {
        self.remote.insert(local_group, remote_group, handle_addr);
    }

    pub fn get(&self, mut addr: Addr) -> Option<ObjectRef<'_>> {
        #[cfg(feature = "network")]
        if addr.is_remote() {
            addr = self.remote.get(addr)?;
        }

        self.local.get(addr.into_bits() as usize)
    }

    pub fn get_owned(&self, mut addr: Addr) -> Option<ObjectArc> {
        #[cfg(feature = "network")]
        if addr.is_remote() {
            addr = self.remote.get(addr)?;
        }

        self.local.clone().get_owned(addr.into_bits() as usize)
    }

    pub(crate) fn vacant_entry(&self, group_no: GroupNo) -> VacantEntry<'_> {
        self.local
            .vacant_entry()
            .map(|entry| VacantEntry { entry, group_no })
            .expect("too many actors")
    }

    pub(crate) fn remove(&self, addr: Addr) {
        self.local.remove(addr.into_bits() as usize);
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

cfg_network!({
    use arc_swap::ArcSwap;
    use fxhash::FxHashMap;

    #[derive(Default)]
    pub(super) struct RemoteToHandleMap {
        // (local_group_no, remote_node_no_group_no) -> handle_addr
        map: ArcSwap<FxHashMap<u64, Addr>>,
    }

    impl RemoteToHandleMap {
        pub(super) fn insert(
            &self,
            local_group: GroupNo,
            remote_group: (NodeNo, GroupNo),
            handle_addr: Addr,
        ) {
            let key = u64::from(local_group) << 32
                | u64::from(remote_group.0) << 8
                | u64::from(remote_group.1);

            self.map.rcu(|map| {
                let mut map = (**map).clone();
                map.insert(key, handle_addr);
                map
            });
        }

        pub(super) fn get(&self, remote_addr: Addr) -> Option<Addr> {
            debug_assert!(remote_addr.is_remote());

            let local = crate::scope::with(|scope| scope.group()).node_no_group_no();
            let remote = remote_addr.node_no_group_no();
            let key = u64::from(local) << 32 | u64::from(remote);

            self.map.load().get(&key).copied()
        }
    }
});
