use fxhash::FxHashMap;
use parking_lot::Mutex;

use elfo_core::{
    addr::{NodeLaunchId, NodeNo},
    topology::Topology,
};

use crate::protocol::internode::GroupInfo;

// TODO: move to discovery?

pub(crate) struct NodeMap {
    pub(crate) nodes: Mutex<FxHashMap<NodeNo, NodeInfo>>,
    pub(crate) this: NodeInfo,
}

impl NodeMap {
    pub(crate) fn new(topology: &Topology) -> Self {
        let this = NodeInfo {
            node_no: topology.node_no(),
            launch_id: topology.launch_id(),
            groups: topology
                .locals()
                .map(|group| {
                    let interests = topology
                        .connections()
                        .filter_map(|conn| {
                            (conn.from == group.addr)
                                .then(|| conn.to.into_remote())
                                .flatten()
                        })
                        .collect();

                    GroupInfo {
                        group_no: group.addr.group_no().expect("invalid group no"),
                        name: group.name,
                        interests,
                    }
                })
                .collect(),
        };

        Self {
            nodes: Default::default(),
            this,
        }
    }
}

#[derive(Clone)]
pub(crate) struct NodeInfo {
    pub(crate) node_no: NodeNo,
    pub(crate) launch_id: NodeLaunchId,
    pub(crate) groups: Vec<GroupInfo>,
}
