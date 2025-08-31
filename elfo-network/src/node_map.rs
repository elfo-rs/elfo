use fxhash::FxHashMap;
use parking_lot::Mutex;

use elfo_core::{
    addr::{GroupNo, NodeLaunchId, NodeNo},
    topology::Topology,
};

use crate::protocol::{internode::GroupInfo, GroupMeta};

// TODO: move to discovery?

pub(crate) struct NodeMap {
    pub(crate) nodes: Mutex<FxHashMap<NodeNo, NodeInfo>>,
    pub(crate) this: NodeInfo,
}

impl NodeMap {
    #[cfg(test)]
    pub(crate) fn empty(node_no: NodeNo, launch_id: NodeLaunchId) -> Self {
        Self {
            nodes: Mutex::new(Default::default()),
            this: NodeInfo::empty(node_no, launch_id),
        }
    }

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

    pub(crate) fn local_group_meta(&self, group_no: GroupNo) -> Option<GroupMeta> {
        make_group_meta(&self.this, group_no)
    }

    pub(crate) fn remote_group_meta(
        &self,
        node_no: NodeNo,
        group_no: GroupNo,
    ) -> Option<GroupMeta> {
        make_group_meta(self.nodes.lock().get(&node_no)?, group_no)
    }
}

#[derive(Clone)]
pub(crate) struct NodeInfo {
    pub(crate) node_no: NodeNo,
    pub(crate) launch_id: NodeLaunchId,
    pub(crate) groups: Vec<GroupInfo>,
}

#[cfg(test)]
impl NodeInfo {
    pub(crate) fn empty(node_no: NodeNo, launch_id: NodeLaunchId) -> Self {
        Self {
            node_no,
            launch_id,
            groups: vec![],
        }
    }
}

fn make_group_meta(node: &NodeInfo, group_no: GroupNo) -> Option<GroupMeta> {
    node.groups
        .iter()
        .find(|g| g.group_no == group_no)
        .map(|g| g.name.clone())
        .map(|group_name| GroupMeta {
            node_no: node.node_no,
            group_no,
            group_name,
        })
}
