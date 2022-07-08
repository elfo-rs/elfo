use elfo_core::{node::NodeNo, GroupNo, MoveOwnership};
use elfo_macros::message;

// Internal.

// TODO

// Internode.

#[message(elfo = elfo_core)]
pub(crate) struct Greeting {
    pub(crate) node_no: NodeNo,
    pub(crate) groups: Vec<RemoteGroupInfo>,
}

#[message(part, elfo = elfo_core)]
pub(crate) struct RemoteGroupInfo {
    pub(crate) group_no: GroupNo,
    pub(crate) name: String,
}

#[message(elfo = elfo_core)]
pub(crate) struct Heartbeat; // TODO: payload for RTT

#[message(elfo = elfo_core)]
pub(crate) struct ConnectToGroup {
    pub(crate) group_no: GroupNo,
}
