use fxhash::FxHashMap;
use parking_lot::Mutex;

use elfo_core::{
    message,
    node::{self, NodeNo},
    Topology,
};

use crate::protocol::internode::GroupInfo;

// TODO: stop sharing? It seems to be useful only for the discovery actor.

pub(crate) struct NodeMap {
    pub(crate) nodes: Mutex<FxHashMap<NodeNo, NodeInfo>>,
    pub(crate) this: NodeInfo,
}

impl NodeMap {
    pub(crate) fn new(topology: &Topology) -> Self {
        let this = NodeInfo {
            node_no: node::node_no(),
            launch_id: LaunchId::generate(),
            groups: topology
                .locals()
                .map(|group| GroupInfo {
                    group_no: group.addr.group_no(),
                    name: group.name,
                })
                .collect(),
            interests: topology.remotes().map(|group| group.name.clone()).collect(),
        };

        Self {
            nodes: Default::default(),
            this,
        }
    }
}

pub(crate) struct NodeInfo {
    pub(crate) node_no: NodeNo,
    pub(crate) launch_id: LaunchId,
    pub(crate) groups: Vec<GroupInfo>,
    pub(crate) interests: Vec<String>,
}

#[message(part, transparent)]
#[derive(Copy, PartialEq, Eq)]
pub(crate) struct LaunchId(u64);

impl LaunchId {
    pub(crate) fn generate() -> Self {
        use std::{
            collections::hash_map::RandomState,
            hash::{BuildHasher, Hasher},
        };

        // `RandomState` is randomly seeded.
        let mut hasher = RandomState::new().build_hasher();
        hasher.write_u32(42);
        Self(hasher.finish())
    }
}

#[test]
fn launch_id_is_random() {
    assert_ne!(LaunchId::generate(), LaunchId::generate());
}
