use std::sync::Arc;

use idr_ebr::{Guard as EbrGuard, Idr};

use crate::{
    addr::{Addr, GroupNo, IdrConfig, NodeLaunchId, NodeNo},
    object::{BorrowedObject, Object, OwnedObject},
};

// Reexported in `_priv`.
#[derive(Clone)]
pub struct AddressBook {
    launch_id: NodeLaunchId,
    local: Arc<Idr<Object, IdrConfig>>,
    #[cfg(feature = "network")]
    remote: Arc<RemoteToHandleMap>, // TODO: use `arc_swap::cache::Cache` in TLS?
}

assert_impl_all!(AddressBook: Sync);

impl AddressBook {
    pub(crate) fn new(launch_id: NodeLaunchId) -> Self {
        let local = Arc::new(Idr::new());

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
        network_actor_addr: Addr,
        local_group: GroupNo,
        remote_group: (NodeNo, GroupNo),
        handle_addr: Addr,
    ) {
        self.remote
            .insert(network_actor_addr, local_group, remote_group, handle_addr);
    }

    #[cfg(feature = "network")]
    pub(crate) fn deregister_remote(
        &self,
        network_actor_addr: Addr,
        local_group: GroupNo,
        remote_group: (NodeNo, GroupNo),
        handle_addr: Addr,
    ) {
        self.remote
            .remove(network_actor_addr, local_group, remote_group, handle_addr);
    }

    #[inline]
    pub fn get<'g>(&self, addr: Addr, guard: &'g EbrGuard) -> Option<BorrowedObject<'g>> {
        let addr = self.prepare_addr(addr)?;

        self.local
            .get(addr.slot_key(self.launch_id)?, guard)
            // idr-ebr doesn't check top bits, so we need to check them manually.
            // It equals to checking the group number, but without extra operations.
            .filter(|object| object.addr() == addr)
    }

    #[inline]
    pub fn get_owned(&self, addr: Addr) -> Option<OwnedObject> {
        let addr = self.prepare_addr(addr)?;

        self.local
            .clone()
            .get_owned(addr.slot_key(self.launch_id)?)
            // idr-ebr doesn't check top bits, so we need to check them manually.
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
        self.local.remove(ward!(addr.slot_key(self.launch_id)));
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

pub(crate) struct VacantEntry<'g> {
    launch_id: NodeLaunchId,
    entry: idr_ebr::VacantEntry<'g, Object, IdrConfig>,
    group_no: GroupNo,
}

impl<'g> VacantEntry<'g> {
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

    #[derive(Clone, Default)]
    struct RemoteToHandleMapInner {
        // (local_group_no, remote_node_no_group_no) -> handle_addr
        map: FxHashMap<u64, Addr>,
        // network_actor_addr -> handle_addr
        fallback: FxHashMap<Addr, Addr>,
    }

    #[derive(Default)]
    pub(super) struct RemoteToHandleMap(ArcSwap<RemoteToHandleMapInner>);

    impl RemoteToHandleMap {
        pub(super) fn insert(
            &self,
            network_actor_addr: Addr,
            local_group: GroupNo,
            remote_group: (NodeNo, GroupNo),
            handle_addr: Addr,
        ) {
            let key = u64::from(local_group.into_bits()) << 32
                | u64::from(remote_group.0.into_bits()) << 8
                | u64::from(remote_group.1.into_bits());

            self.0.rcu(|inner| {
                let mut inner = (**inner).clone();
                inner.map.insert(key, handle_addr);
                inner.fallback.insert(network_actor_addr, handle_addr);
                inner
            });
        }

        pub(super) fn remove(
            &self,
            network_actor_addr: Addr,
            local_group: GroupNo,
            remote_group: (NodeNo, GroupNo),
            handle_addr: Addr,
        ) {
            let key = u64::from(local_group.into_bits()) << 32
                | u64::from(remote_group.0.into_bits()) << 8
                | u64::from(remote_group.1.into_bits());

            self.0.rcu(|inner| {
                // We don't want to remove a handle that was not registered by us.
                let mut inner = (**inner).clone();
                if inner.map.get(&key) == Some(&handle_addr) {
                    inner.map.remove(&key);
                }
                // In the fallback map `network_actor_addr` is unique, so no lookups are needed.
                inner.fallback.remove(&network_actor_addr);
                inner
            });
        }

        pub(super) fn get(&self, remote_addr: Addr) -> Option<Addr> {
            debug_assert!(remote_addr.is_remote());

            let local_actor = crate::scope::with(|scope| scope.actor());
            let remote = remote_addr.node_no_group_no();
            let key = u64::from(local_actor.node_no_group_no()) << 32 | u64::from(remote);

            let inner = self.0.load();
            inner
                .map
                .get(&key)
                .or_else(|| inner.fallback.get(&local_actor))
                .copied()
        }
    }
});
