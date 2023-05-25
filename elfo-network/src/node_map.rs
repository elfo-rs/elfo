use fxhash::FxHashMap;
use parking_lot::Mutex;

use elfo_core::{
    node::{self, NodeNo},
    GroupNo, Topology,
};

pub(crate) struct NodeMap {
    pub(crate) nodes: Mutex<FxHashMap<NodeNo, NodeInfo>>,
    pub(crate) this: NodeInfo,
}

impl NodeMap {
    pub(crate) fn new(topology: &Topology) -> Self {
        let this = NodeInfo {
            node_no: node::node_no(),
            groups: topology
                .locals()
                .map(|group| GroupInfo {
                    group_no: group.addr.group_no(),
                    name: group.name,
                    status: Status::Live,
                })
                .collect(),
            status: Status::Live,
        };

        Self {
            nodes: Default::default(),
            this,
        }
    }
}

pub(crate) struct NodeInfo {
    pub(crate) node_no: NodeNo,
    pub(crate) groups: Vec<GroupInfo>,
    // TODO: pub(crate) path: String,
    pub(crate) status: Status,
}

pub(crate) struct GroupInfo {
    pub(crate) group_no: GroupNo,
    pub(crate) name: String,
    pub(crate) status: Status,
}

pub(crate) enum Status {
    Unknown,
    Live, // TODO: last_heartbeat
}
