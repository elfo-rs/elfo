use std::sync::Arc;

use sharded_slab::{self as slab, Slab};

use crate::{
    addr::{Addr, GroupNo, NodeLaunchId, NodeNo, SlabConfig},
    object::{Object, ObjectArc, ObjectRef},
};

// Reexported in `_priv`.
#[derive(Clone)]
pub struct AddressBook {
    launch_id: NodeLaunchId,
    local: Arc<Slab<Object, SlabConfig>>,
    #[cfg(feature = "network")]
    remote: Arc<RemoteToHandleMap>, // TODO: use `arc_swap::cache::Cache` in TLS?
}

assert_impl_all!(AddressBook: Sync);

impl AddressBook {
    pub(crate) fn new(launch_id: NodeLaunchId) -> Self {
        let local = Arc::new(Slab::new_with_config::<SlabConfig>());

        #[cfg(feature = "network")]
        return Self {
            launch_id,
            local,
            remote: Default::default(),
        };

        #[cfg(not(feature = "network"))]
        Self { launch_id, local }
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

    pub fn get(&self, addr: Addr) -> Option<ObjectRef<'_>> {
        let addr = self.prepare_addr(addr)?;

        self.local
            .get(addr.slot_key(self.launch_id))
            // sharded-slab doesn't check top bits, so we need to check them manually.
            // It equals to checking the group number, but without extra operations.
            .filter(|object| object.addr() == addr)
    }

    pub fn get_owned(&self, addr: Addr) -> Option<ObjectArc> {
        let addr = self.prepare_addr(addr)?;

        self.local
            .clone()
            .get_owned(addr.slot_key(self.launch_id))
            // sharded-slab doesn't check top bits, so we need to check them manually.
            // It equals to checking the group number, but without extra operations.
            .filter(|object| object.addr() == addr)
    }

    pub(crate) fn vacant_entry(&self, group_no: GroupNo) -> VacantEntry<'_> {
        self.local
            .vacant_entry()
            .map(|entry| VacantEntry {
                launch_id: self.launch_id,
                entry,
                group_no,
            })
            .expect("too many actors")
    }

    pub(crate) fn remove(&self, addr: Addr) {
        self.local.remove(addr.into_bits() as usize);
    }

    #[inline(always)]
    fn prepare_addr(&self, addr: Addr) -> Option<Addr> {
        // If the address is null, return `None`.
        // It's required, because `Addr::NULL.slot_key()` can be valid for sharded-slab.
        if addr.is_null() {
            return None;
        }

        // If the address is remote, replace it with a remote handler's address.
        #[cfg(feature = "network")]
        if addr.is_remote() {
            return self.remote.get(addr);
        }

        Some(addr)
    }
}

pub(crate) struct VacantEntry<'b> {
    launch_id: NodeLaunchId,
    entry: slab::VacantEntry<'b, Object, SlabConfig>,
    group_no: GroupNo,
}

impl<'b> VacantEntry<'b> {
    pub(crate) fn insert(self, object: Object) {
        self.entry.insert(object)
    }

    pub(crate) fn addr(&self) -> Addr {
        Addr::new_local(self.entry.key(), self.group_no, self.launch_id)
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
            let key = u64::from(local_group.into_bits()) << 32
                | u64::from(remote_group.0.into_bits()) << 8
                | u64::from(remote_group.1.into_bits());

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
