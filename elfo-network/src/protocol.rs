use elfo_core::{message, node::NodeNo, GroupNo, MoveOwnership};

use crate::{connection::Connection, node_map::LaunchId};

// Internal.

#[message]
pub(crate) struct StartTransmitter {
    pub(crate) node_no: NodeNo,
    pub(crate) remote_group_no: GroupNo,
    pub(crate) connection: MoveOwnership<Connection>,
}

#[message]
pub(crate) struct StartReceiver {
    pub(crate) node_no: NodeNo,
    pub(crate) local_group_no: GroupNo,
    pub(crate) connection: MoveOwnership<Connection>,
}

pub(crate) mod internode {
    use super::*;

    // client                           server
    //             any connection
    //      ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
    //      Handshake -->   <-- Handshake
    //                  ...
    //
    //           control connection
    //      ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
    //                  ...
    //      NodeCompositionReport -->
    //          <-- NodeCompositionReport
    //                  ...
    //
    //            data connection
    //      ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
    //                  ...
    //      ReadyToReceive -->
    //                <-- ReadyToTransmit
    //                  ...
    //
    //            data connection
    //      ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
    //                  ...
    //      ReadyToTransmit -->
    //                 <-- ReadyToReceive
    //                  ...
    //
    //             any connection
    //      ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
    //                  ...
    //      Ping -->
    //                           <-- Pong
    //                  ...
    //
    //             any connection
    //      ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
    //                  ...
    //                           <-- Ping
    //      Pong -->
    //                  ...
    //
    // TODO: close, status changes.

    #[message]
    pub(crate) struct Handshake {
        pub(crate) node_no: NodeNo,
        pub(crate) launch_id: LaunchId,
    }

    #[message]
    pub(crate) struct NodeCompositionReport {
        pub(crate) groups: Vec<GroupInfo>,
        /// Remote group's names that this node is interested in.
        pub(crate) interests: Vec<String>,
    }

    #[message(part)]
    pub(crate) struct GroupInfo {
        pub(crate) group_no: GroupNo, // TODO: just `no`?
        pub(crate) name: String,
    }

    #[message]
    pub(crate) struct ReadyToReceive {
        /// Local group's number of a client.
        pub(crate) group_no: GroupNo,
    }

    #[message]
    pub(crate) struct ReadyToTransmit {
        /// Local group's number of a server.
        pub(crate) group_no: GroupNo,
    }

    #[message]
    pub(crate) struct Ping {
        pub(crate) payload: u64,
    }

    #[message]
    pub(crate) struct Pong {
        pub(crate) payload: u64,
    }
}
