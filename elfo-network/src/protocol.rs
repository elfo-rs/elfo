use elfo_core::{
    addr::{GroupNo, NodeNo},
    message, MoveOwnership,
};

use crate::{codec::format::NetworkAddr, config::Transport, socket::Socket};

// Internal.

#[message]
pub(crate) struct HandleConnection {
    pub(crate) local: GroupInfo,
    pub(crate) remote: GroupInfo,
    pub(crate) socket: MoveOwnership<Socket>,
    /// Initial window size of every flow.
    pub(crate) initial_window: i32,
    // TODO: different windows for rx/tx and routed flows.
    pub(crate) transport: Option<Transport>,
}

#[message]
pub(crate) struct DataConnectionFailed {
    pub(crate) transport: Transport,
    pub(crate) local: GroupNo,
    pub(crate) remote: (NodeNo, GroupNo),
}

#[message(part)]
#[derive(PartialEq, Eq, Hash)]
pub(crate) struct GroupInfo {
    pub(crate) node_no: NodeNo,
    pub(crate) group_no: GroupNo,
    pub(crate) group_name: String,
}

pub(crate) mod internode {
    use super::*;

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
    //      UpdateFlow -->
    //                  ...
    //                     <-- UpdateFlow
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
        /// Initial window size for every flow.
        pub(crate) initial_window: i32,
        // TODO: different windows for rx/tx and routed flows.
    }

    #[message]
    pub(crate) struct UpdateFlow {
        pub(crate) addr: NetworkAddr,
        pub(crate) window_delta: i32,
    }

    #[message]
    pub(crate) struct CloseFlow {
        pub(crate) addr: NetworkAddr,
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
