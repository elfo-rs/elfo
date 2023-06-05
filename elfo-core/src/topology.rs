use std::{cell::RefCell, sync::Arc};

use parking_lot::RwLock;
use sealed::sealed;
use tokio::runtime::Handle;

#[cfg(feature = "unstable-stuck-detection")]
use crate::stuck_detection::StuckDetector;
use crate::{
    address_book::{AddressBook, VacantEntry},
    context::Context,
    demux::Demux,
    envelope::Envelope,
    group::Blueprint,
    object::Object,
    runtime::RuntimeManager,
    Addr, GroupNo,
};

/// The topology defines local and remote groups, and routes between them.
#[derive(Clone)]
pub struct Topology {
    pub(crate) book: AddressBook,
    inner: Arc<RwLock<Inner>>,
}

#[derive(Default)]
struct Inner {
    locals: Vec<LocalActorGroup>,
    #[cfg(feature = "network")]
    remotes: Vec<RemoteActorGroup>,
    connections: Vec<Connection>,
    rt_manager: RuntimeManager,
}

/// Represents a local group.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct LocalActorGroup {
    pub addr: Addr,
    pub name: String,
    pub is_entrypoint: bool,
}

/// Represents a connection between two groups.
#[stability::unstable]
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Connection {
    pub from: Addr,
    pub to: ConnectionTo,
}

#[stability::unstable]
#[derive(Debug, Clone)]
pub enum ConnectionTo {
    Local(Addr),
    #[cfg(feature = "network")]
    Remote(String),
}

impl ConnectionTo {
    #[stability::unstable]
    pub fn into_remote(self) -> Option<String> {
        match self {
            Self::Local(_) => None,
            #[cfg(feature = "network")]
            Self::Remote(name) => Some(name),
        }
    }
}

impl Default for Topology {
    fn default() -> Self {
        Self::empty()
    }
}

impl Topology {
    /// Creates a new empty topology.
    pub fn empty() -> Self {
        Self {
            book: AddressBook::new(),
            inner: Arc::new(RwLock::new(Inner::default())),
        }
    }

    #[stability::unstable]
    pub fn add_dedicated_rt<F: Fn(&crate::ActorMeta) -> bool + Send + Sync + 'static>(
        &self,
        filter: F,
        handle: Handle,
    ) {
        self.inner.write().rt_manager.add(filter, handle);
    }

    #[cfg(feature = "unstable-stuck-detection")]
    pub fn stuck_detector(&self) -> StuckDetector {
        self.inner.read().rt_manager.stuck_detector()
    }

    /// Declares a new local group.
    ///
    /// # Panics
    /// * If the name is already taken for another local group.
    /// * If there are too many local groups.
    #[track_caller]
    pub fn local(&self, name: impl Into<String>) -> Local<'_> {
        let name = name.into();
        let mut inner = self.inner.write();

        for local in &inner.locals {
            if local.name == name {
                panic!("local group name `{name}` is already taken");
            }
        }

        let group_no = inner.locals.len() + 1; // 0 is reserved for `system.init`.

        // `GroupNo::MAX` is reserved for `Addr::NULL`, so we cannot use it.
        if group_no == usize::from(GroupNo::MAX) {
            panic!("too many groups");
        }

        let entry = self.book.vacant_entry(group_no as GroupNo);
        inner.locals.push(LocalActorGroup {
            addr: entry.addr(),
            name: name.clone(),
            is_entrypoint: false,
        });

        Local {
            name,
            topology: self,
            entry,
            demux: RefCell::new(Demux::default()),
        }
    }

    /// Returns an iterator over all local groups.
    pub fn locals(&self) -> impl Iterator<Item = LocalActorGroup> + '_ {
        let inner = self.inner.read();
        inner.locals.clone().into_iter()
    }

    #[stability::unstable]
    pub fn connections(&self) -> impl Iterator<Item = Connection> + '_ {
        let inner = self.inner.read();
        inner.connections.clone().into_iter()
    }
}

/// Represents a local group's settings.
#[must_use]
pub struct Local<'t> {
    topology: &'t Topology,
    name: String,
    entry: VacantEntry<'t>,
    demux: RefCell<Demux>,
}

impl<'t> Local<'t> {
    #[doc(hidden)]
    pub fn addr(&self) -> Addr {
        self.entry.addr()
    }

    /// Mark this group as an entrypoint.
    ///
    /// It means, that this group will be started automatically when the system
    /// starts, with empty configuration is provided.
    ///
    /// Usually, only `system.configurers` group is marked as an entrypoint.
    pub fn entrypoint(self) -> Self {
        let mut inner = self.topology.inner.write();
        let group = inner
            .locals
            .iter_mut()
            .find(|group| group.addr == self.entry.addr())
            .expect("just created");
        group.is_entrypoint = true;
        self
    }

    /// Defines a route to the given destination (local or remote group).
    ///
    /// # Examples
    /// Local to local:
    /// ```
    /// # use elfo_core as elfo;
    /// # #[elfo::message] struct SomeEvent;
    /// use elfo::{Topology, msg};
    ///
    /// let topology = Topology::empty();
    /// let foo = topology.local("foo");
    /// let bar = topology.local("bar");
    ///
    /// foo.route_to(&bar, |envelope| {
    ///     msg!(match envelope {
    ///         SomeEvent => true,
    ///         _ => false,
    ///     })
    /// });
    /// ```
    ///
    /// Local to remote (requires the `network` feature): TODO
    pub fn route_to<F>(&self, dest: &impl Destination<F>, filter: F) {
        dest.extend_demux(
            self.entry.addr().group_no(),
            &mut self.demux.borrow_mut(),
            filter,
        );

        let mut inner = self.topology.inner.write();
        inner.connections.push(Connection {
            from: self.entry.addr(),
            to: dest.connection_endpoint(),
        });
    }

    // TODO: deprecate?
    pub fn route_all_to(&self, dest: &Local<'_>) {
        let addr = dest.entry.addr();
        self.demux
            .borrow_mut()
            .append(move |_, addrs| addrs.push(addr));
    }

    /// Mounts a blueprint to this group.
    pub fn mount(self, blueprint: Blueprint) {
        let addr = self.entry.addr();
        let book = self.topology.book.clone();
        let ctx = Context::new(book, self.demux.into_inner()).with_group(addr);
        let rt_manager = self.topology.inner.read().rt_manager.clone();
        let object = (blueprint.run)(ctx, self.name, rt_manager);
        self.entry.insert(object);
    }
}

#[sealed]
pub trait Destination<F> {
    #[doc(hidden)]
    fn extend_demux(&self, source_group_no: GroupNo, demux: &mut Demux, filter: F);

    #[doc(hidden)]
    fn connection_endpoint(&self) -> ConnectionTo;
}

#[sealed]
impl<F> Destination<F> for Local<'_>
where
    F: Fn(&Envelope) -> bool + Send + Sync + 'static,
{
    fn extend_demux(&self, _: GroupNo, demux: &mut Demux, filter: F) {
        let addr = self.entry.addr();
        demux.append(move |envelope, addrs| {
            if filter(envelope) {
                addrs.push(addr);
            }
        });
    }

    fn connection_endpoint(&self) -> ConnectionTo {
        ConnectionTo::Local(self.entry.addr())
    }
}

cfg_network!({
    use arc_swap::ArcSwap;
    use fxhash::FxHashMap;

    use crate::{node::NodeNo, remote::RemoteHandle};

    type Nodes = Arc<ArcSwap<FxHashMap<NodeNo, Addr>>>;

    // TODO: remove `Clone` here, possible footgun in the future.
    /// Represents remote group(s).
    #[stability::unstable]
    #[derive(Debug, Clone)]
    #[non_exhaustive]
    pub struct RemoteActorGroup {
        pub name: String,
        nodes: FxHashMap<GroupNo, Nodes>,
    }

    impl Topology {
        /// # Panics
        /// If the name isn't used in the topology.
        #[stability::unstable]
        pub fn register_remote(
            &self,
            local_group: GroupNo,
            remote_group: (NodeNo, GroupNo),
            remote_group_name: &str,
            handle: impl RemoteHandle,
        ) -> RegisterRemoteGroupGuard {
            // XXX: get rid of `MAX` here.
            let entry = self.book.vacant_entry(GroupNo::MAX);
            let handle_addr = entry.addr();
            let object = Object::new(handle_addr, Box::new(handle) as Box<dyn RemoteHandle>);
            entry.insert(object);

            self.book
                .register_remote(local_group, remote_group, handle_addr);

            let inner = self.inner.write();
            let group = inner
                .remotes
                .iter()
                .find(|group| group.name == remote_group_name)
                .expect("remote group not found");

            group.nodes[&local_group].rcu(|nodes| {
                let mut nodes = (**nodes).clone();
                nodes.insert(remote_group.0, handle_addr);
                nodes
            });

            RegisterRemoteGroupGuard(())
        }

        /// Declares a new remote group.
        ///
        /// # Panics
        /// * If the name is already taken for another remote group.
        pub fn remote(&self, name: impl Into<String>) -> Remote<'_> {
            let name = name.into();
            let mut inner = self.inner.write();

            for remote in &inner.remotes {
                if remote.name == name {
                    panic!("remote group name `{name}` is already taken");
                }
            }

            inner.remotes.push(RemoteActorGroup {
                name: name.clone(),
                nodes: Default::default(),
            });

            Remote {
                topology: self,
                name,
            }
        }

        /// Returns an iterator over all remote groups.
        #[stability::unstable]
        pub fn remotes(&self) -> impl Iterator<Item = RemoteActorGroup> + '_ {
            let inner = self.inner.read();
            inner.remotes.clone().into_iter()
        }
    }

    /// Represents a remote group's settings.
    pub struct Remote<'t> {
        topology: &'t Topology,
        name: String,
    }

    #[sealed]
    impl<F> Destination<F> for Remote<'_>
    where
        F: Fn(&Envelope, &NodeDiscovery) -> Outcome + Send + Sync + 'static,
    {
        fn extend_demux(&self, local_group_no: GroupNo, demux: &mut Demux, filter: F) {
            let nodes = self
                .topology
                .inner
                .write()
                .remotes
                .iter_mut()
                .find(|group| group.name == self.name)
                .expect("remote group not found")
                .nodes
                .entry(local_group_no)
                .or_default()
                .clone();

            demux.append(move |envelope, addrs| {
                let discovery = NodeDiscovery(());

                match filter(envelope, &discovery) {
                    Outcome::Unicast(node_no) => {
                        if let Some(addr) = nodes.load().get(&node_no) {
                            addrs.push(*addr);
                        }
                    }
                    Outcome::Multicast(node_nos) => {
                        let nodes = nodes.load();
                        for node_no in node_nos {
                            if let Some(addr) = nodes.get(&node_no) {
                                addrs.push(*addr);
                            }
                        }
                    }
                    Outcome::Broadcast => {
                        let nodes = nodes.load();
                        for addr in nodes.values() {
                            addrs.push(*addr);
                        }
                    }
                    Outcome::Discard => {}
                }
            });
        }

        fn connection_endpoint(&self) -> ConnectionTo {
            ConnectionTo::Remote(self.name.clone())
        }
    }

    #[derive(Debug)]
    #[non_exhaustive]
    pub enum Outcome {
        /// Routes a message to the specified node.
        Unicast(NodeNo),
        /// Routes a message to all specified nodes.
        Multicast(Vec<NodeNo>),
        /// Routes a message to all active nodes.
        Broadcast,
        /// Discards a message.
        Discard,
    }

    // Nothing for now, reserved for future use.
    pub struct NodeDiscovery(());

    #[stability::unstable]
    pub struct RegisterRemoteGroupGuard(());

    impl Drop for RegisterRemoteGroupGuard {
        fn drop(&mut self) {
            // TODO: unregister
        }
    }
});
