use elfo_core::{message, node::NodeNo, GroupNo, MoveOwnership};

use crate::{node_map::LaunchId, socket::Socket};

// Internal.

#[message]
pub(crate) struct HandleConnection {
    pub(crate) local: GroupNo,
    pub(crate) remote: (NodeNo, GroupNo),
    pub(crate) socket: MoveOwnership<Socket>,
}

pub(crate) mod internode {
    use super::*;

    //             any connection
    //      ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
    //      Handshake -->   <-- Handshake
    //                  ...
    //
    //           control connection
    //      ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
    //      (client)             (server)
    //                  ...
    //      SwitchToControl -->
    //                <-- SwitchToControl
    //                  ...
    //
    //            data connection
    //      ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
    //      (client)             (server)
    //                  ...
    //      SwitchToData -->
    //                   <-- SwitchToData
    //                  ...
    //
    //             any connection
    //      ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
    //                  ...
    //      Ping -->
    //                           <-- Pong
    //                  ...
    //
    // TODO: close, status changes.

    #[message]
    pub(crate) struct Handshake {
        pub(crate) node_no: NodeNo,
        pub(crate) launch_id: LaunchId,
    }

    #[message]
    pub(crate) struct SwitchToControl {
        pub(crate) groups: Vec<GroupInfo>,
    }

    #[message(part)]
    pub(crate) struct GroupInfo {
        pub(crate) group_no: GroupNo, // TODO: just `no`?
        pub(crate) name: String,
        /// Remote group's names that this group is interested in.
        pub(crate) interests: Vec<String>,
    }

    #[message]
    pub(crate) struct SwitchToData {
        /// Local group's number of a client.
        pub(crate) my_group_no: GroupNo,
        /// Local group's number of a server.
        pub(crate) your_group_no: GroupNo,
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
