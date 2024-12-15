use std::{
    fmt,
    num::{NonZeroU16, NonZeroU8},
};

use derive_more::Display;
use idr_ebr::Key;
use serde::{Deserialize, Serialize};

// === NodeNo ===

/// Represents the node's number in a distributed system.
/// Cannot be `0`, it's reserved to represent the local node.
///
/// Nodes with the same `node_no` cannot be connected.
///
/// NOTE: It's 16-bit unsigned integer, which requires manual management for
/// bigger-than-small clusters and will be replaced with [`NodeLaunchId`]
/// totally in the future in order to simplify the management.
#[stability::unstable]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[derive(Display, Serialize, Deserialize)]
pub struct NodeNo(NonZeroU16);

impl NodeNo {
    pub(crate) fn generate() -> Self {
        Self::from_bits((random_u64() as u16).max(1)).unwrap()
    }

    #[inline]
    pub const fn from_bits(bits: u16) -> Option<Self> {
        match NonZeroU16::new(bits) {
            Some(node_no) => Some(Self(node_no)),
            None => None,
        }
    }

    #[inline]
    pub const fn into_bits(self) -> u16 {
        self.0.get()
    }
}

// === NodeLaunchId ===

/// Randomly generated identifier at the node start.
///
/// Used for several purposes:
/// * To distinguish between different launches of the same node.
/// * To detect reusing of the same node no.
/// * To improve [`Addr`] uniqueness in the cluster.
#[stability::unstable]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Display)]
pub struct NodeLaunchId(u64);

impl NodeLaunchId {
    pub(crate) fn generate() -> Self {
        Self(random_u64())
    }

    #[stability::unstable]
    #[inline]
    pub fn from_bits(bits: u64) -> Self {
        Self(bits)
    }

    #[stability::unstable]
    #[inline]
    pub fn into_bits(self) -> u64 {
        self.0
    }
}

// === GroupNo ===

/// Represents the actor group's number.
///
/// Cannot be `0`, it's reserved to represent `Addr::NULL` unambiguously.
/// XORed with random [`NodeLaunchId`] if the `network` feature is enabled.
#[stability::unstable]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[derive(Display, Serialize, Deserialize)]
pub struct GroupNo(NonZeroU8);

impl GroupNo {
    #[cfg(feature = "network-2")] // TODO(loyd): enable after fixing reconnects
    pub(crate) fn new(no: u8, launch_id: NodeLaunchId) -> Option<Self> {
        if no == 0 {
            return None;
        }

        let xor = (launch_id.into_bits() >> GROUP_NO_SHIFT) as u8;

        // `no = 0` is forbidden, thus there is no mapping to just `xor`.
        let group_no = if no != xor { no ^ xor } else { xor };

        Some(Self(NonZeroU8::new(group_no).unwrap()))
    }

    #[cfg(not(feature = "network-2"))]
    pub(crate) fn new(no: u8, _launch_id: NodeLaunchId) -> Option<Self> {
        NonZeroU8::new(no).map(Self)
    }

    #[stability::unstable]
    #[inline]
    pub fn from_bits(bits: u8) -> Option<Self> {
        NonZeroU8::new(bits).map(Self)
    }

    #[stability::unstable]
    #[inline]
    pub fn into_bits(self) -> u8 {
        self.0.get()
    }
}

// === Addr ===

/// Represents the global, usually unique address of an actor or a group.
///
/// # Uniqueness
///
/// An address is based on an [IDR] to make it a simple sendable number
/// (as opposed to reference counting) and provide better performance of lookups
/// than hashmaps. However, it means deletions and insertions to the same
/// underlying slot multiple times can lead to reusing the address for a
/// different actor.
///
/// Elfo tries to do its best to ensure the uniqueness of this value:
/// * Alive actors on the same node always have different addresses.
/// * Actors in different nodes have different address spaces.
/// * Actors in different groups have different address spaces.
/// * An address includes the version number to guard against the [ABA] problem.
/// * An address is randomized between restarts of the same node if the
///   `network` feature is enabled.
///
/// # Using addresses in messages
///
/// The current implementation of network depends on the fact that
/// `Addr` cannot be sent inside messages. It prevents from different
/// possible errors like responding without having a valid connection.
/// The only way to get an address of remote actor is `envelope.sender()`.
/// If sending `Addr` inside a message is unavoidable, use `Local<Addr>`,
/// however it won't be possible to send such message to a remote actor.
///
/// [IDR]: https://crates.io/crates/idr-ebr
/// [ABA]: https://en.wikipedia.org/wiki/ABA_problem
// ~
// The structure:
//  64           48         40           25                       0
//  ┌────────────┬──────────┬────────────┬────────────────────────┐
//  │   node_no  │ group_no │ generation │    page_no + slot_no   │
//  │     16b    │    8b    │     15b    │           25b          │
//  └────────────┴──────────┴────────────┴────────────────────────┘
//   (0 if local)           └─────────── IDR key (40b) ───────────┘
//
//
// Limits:
// - max nodes in a cluster                        65'535  (1)
// - max groups in a node                             255  (2, 3)
// - max active actors                         33'554'400
// - slot generations to prevent ABA               32'768
//
// 1. `0` is reserved to represent the local node.
// 2. `0` is reserved to represent `Addr::NULL` unambiguously.
// 3. at least one group (for `system.init`) is always present.
//
// If the `network` feature is enabled, bottom 48 bits are XORed with the current node's launch
// number, generated at startup. It ensures that the same actor on different launches of the same
// node will have different addresses. The original address is never printed or even represented
// and the slot key part is restored only by calling private `Addr::slot_key(launch_no)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Addr(u64); // TODO: make it `NonZeroU64` instead of `Addr::NULL`

const NODE_NO_SHIFT: u32 = 48;
const GROUP_NO_SHIFT: u32 = 40;

// See `Addr` docs for details.
assert_not_impl_all!(Addr: Serialize, Deserialize<'static>);

impl fmt::Display for Addr {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_null() {
            return f.write_str("null");
        }

        let group_no = self.group_no().expect("invalid addr");
        let bottom = self.0 & ((1 << GROUP_NO_SHIFT) - 1);

        if let Some(node_no) = self.node_no() {
            write!(f, "{}/{}/{}", node_no, group_no, bottom)
        } else {
            write!(f, "{}/{}", group_no, bottom)
        }
    }
}

impl Addr {
    #[stability::unstable]
    pub const NULL: Addr = Addr(0);

    #[cfg(feature = "network")]
    pub(crate) fn new_local(slot_key: Key, group_no: GroupNo, launch_id: NodeLaunchId) -> Self {
        let slot_key = u64::from(slot_key);
        debug_assert!(slot_key < (1 << GROUP_NO_SHIFT));
        let slot_key = (slot_key ^ launch_id.into_bits()) & ((1 << GROUP_NO_SHIFT) - 1);
        Self::new_local_inner(slot_key, group_no)
    }

    #[cfg(not(feature = "network"))]
    pub(crate) fn new_local(slot_key: Key, group_no: GroupNo, _launch_id: NodeLaunchId) -> Self {
        let slot_key = u64::from(slot_key);
        debug_assert!(slot_key < (1 << GROUP_NO_SHIFT));
        Self::new_local_inner(slot_key as u64, group_no)
    }

    fn new_local_inner(slot_key: u64, group_no: GroupNo) -> Self {
        Self(u64::from(group_no.into_bits()) << GROUP_NO_SHIFT | slot_key)
    }

    #[stability::unstable]
    #[inline]
    pub fn from_bits(bits: u64) -> Option<Self> {
        Some(Self(bits)).filter(|addr| addr.is_null() ^ addr.group_no().is_some())
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
        !self.is_null() && self.node_no().is_none()
    }

    #[cfg(feature = "network")]
    #[inline]
    pub fn is_remote(self) -> bool {
        self.node_no().is_some()
    }

    #[stability::unstable]
    #[inline]
    pub fn node_no(self) -> Option<NodeNo> {
        NodeNo::from_bits((self.0 >> NODE_NO_SHIFT) as u16)
    }

    #[stability::unstable]
    #[inline]
    pub fn group_no(self) -> Option<GroupNo> {
        GroupNo::from_bits((self.0 >> GROUP_NO_SHIFT) as u8)
    }

    #[cfg(feature = "network")]
    pub(crate) fn node_no_group_no(self) -> u32 {
        (self.0 >> GROUP_NO_SHIFT) as u32
    }

    #[cfg(feature = "network")]
    pub(crate) fn slot_key(self, launch_id: NodeLaunchId) -> Option<Key> {
        // IDR uses the lower bits only, so we can xor the whole address.
        (self.0 ^ launch_id.into_bits()).try_into().ok()
    }

    #[cfg(not(feature = "network"))]
    pub(crate) fn slot_key(self, _launch_id: NodeLaunchId) -> Option<Key> {
        self.0.try_into().ok()
    }

    #[cfg(feature = "network")]
    #[stability::unstable]
    #[inline]
    pub fn into_remote(self, node_no: NodeNo) -> Self {
        if self.is_local() {
            Self(self.0 | (node_no.into_bits() as u64) << NODE_NO_SHIFT)
        } else {
            self
        }
    }

    #[stability::unstable]
    #[inline]
    pub fn into_local(self) -> Self {
        Self(self.0 & ((1 << NODE_NO_SHIFT) - 1))
    }
}

// === IdrConfig ===

pub(crate) struct IdrConfig;

impl idr_ebr::Config for IdrConfig {
    const INITIAL_PAGE_SIZE: u32 = 32;
    const MAX_PAGES: u32 = 20;
    const RESERVED_BITS: u32 = 24;
}
const_assert_eq!(
    idr_ebr::Idr::<crate::object::Object, IdrConfig>::USED_BITS,
    GROUP_NO_SHIFT
);

// === random_u64 ===

fn random_u64() -> u64 {
    use std::{
        collections::hash_map::RandomState,
        hash::{BuildHasher, Hash, Hasher},
        thread,
        time::Instant,
    };

    let mut hasher = RandomState::new().build_hasher();
    0xE1F0E1F0E1F0E1F0u64.hash(&mut hasher);
    Instant::now().hash(&mut hasher);
    thread::current().id().hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use proptest::prelude::*;

    use super::*;

    #[test]
    fn node_no_generate() {
        for _ in 0..1_000_000 {
            NodeNo::generate();
        }
    }

    #[test]
    fn node_launch_id_generate() {
        let count = 10;
        let set: HashSet<_> = (0..count).map(|_| NodeLaunchId::generate()).collect();
        assert_eq!(set.len(), count);
    }

    #[test]
    fn group_no() {
        let launch_ids = (0..5)
            .map(|_| NodeLaunchId::generate())
            .chain(Some(NodeLaunchId::from_bits(0)))
            .collect::<Vec<_>>();

        for launch_id in launch_ids {
            // no = 0 is always invalid.
            assert_eq!(GroupNo::new(0, launch_id), None);

            // `GroupNo` is unique for any `NodeLaunchId`.
            let set = (1..=u8::MAX)
                .map(|no| GroupNo::new(no, launch_id).unwrap())
                .collect::<HashSet<_>>();

            assert_eq!(set.len(), usize::from(u8::MAX));
        }
    }

    proptest! {
        #[test]
        fn addr(
            slot_keys in prop::collection::hash_set(1u64..(1 << GROUP_NO_SHIFT), 10),
            group_nos in prop::collection::hash_set(1..=u8::MAX, 10),
            launch_ids in prop::collection::hash_set(prop::num::u64::ANY, 10),
        ) {
            #[cfg(feature = "network")]
            let expected_count = slot_keys.len() * group_nos.len() * launch_ids.len();
            #[cfg(not(feature = "network"))]
            let expected_count = slot_keys.len() * group_nos.len();

            let mut set = HashSet::with_capacity(expected_count);

            for slot_key in &slot_keys {
                for group_no in &group_nos {
                    for launch_id in &launch_ids {
                        let slot_key = Key::try_from(*slot_key).unwrap();
                        let launch_id = NodeLaunchId::from_bits(*launch_id);
                        let group_no = GroupNo::new(*group_no, launch_id).unwrap();
                        let addr = Addr::new_local(slot_key, group_no, launch_id);
                        set.insert(addr);

                        prop_assert!(!addr.is_null());
                        prop_assert!(addr.is_local());
                        prop_assert_eq!(addr.group_no(), Some(group_no));
                        prop_assert_eq!(addr.node_no(), None);

                        let actual_slot_key = u64::from(addr.slot_key(launch_id).unwrap());
                        prop_assert_eq!(actual_slot_key & ((1 << GROUP_NO_SHIFT) - 1), u64::from(slot_key));
                        prop_assert_eq!(addr.into_local(), addr);
                        prop_assert_eq!(Addr::from_bits(addr.into_bits()), Some(addr));
                        prop_assert_eq!(addr.to_string().split('/').count(), 2);
                        prop_assert!(addr.to_string().starts_with(&group_no.to_string()));

                        #[cfg(feature = "network")]
                        {
                            prop_assert!(!addr.is_remote());
                            let node_no = NodeNo::from_bits(42).unwrap();
                            let remote = addr.into_remote(node_no);
                            prop_assert!(!remote.is_null());
                            prop_assert!(!remote.is_local());
                            prop_assert!(remote.is_remote());
                            prop_assert_eq!(remote.group_no(), Some(group_no));
                            prop_assert_eq!(remote.node_no(), Some(node_no));
                            prop_assert_eq!(addr.into_local(), addr);
                            prop_assert_eq!(remote.node_no_group_no() >> 8, u32::from(node_no.into_bits()));
                            prop_assert_eq!(remote.node_no_group_no() & 0xff, u32::from(group_no.into_bits()));
                            prop_assert_eq!(remote.to_string().split('/').count(), 3);
                            prop_assert!(remote.to_string().starts_with(&node_no.to_string()));
                        }
                    }
                }
            }

            // Check uniqueness.
            prop_assert_eq!(set.len(), expected_count);
        }
    }

    #[test]
    fn addr_null() {
        let null = Addr::NULL;
        assert_eq!(null.to_string(), "null");
        assert!(null.is_null());
        assert_eq!(null.into_local(), null);
        assert_eq!(null.group_no(), None);
        assert_eq!(null.node_no(), None);
        #[cfg(feature = "network")]
        {
            assert!(!null.is_remote());
            assert_eq!(null.into_remote(NodeNo::from_bits(42).unwrap()), null);
            assert_eq!(null.node_no_group_no(), 0);
        }
    }

    #[test]
    fn addr_invalid() {
        assert_eq!(Addr::from_bits(1), None);
    }
}
