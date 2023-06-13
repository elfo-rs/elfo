use elfo_core::{message, node::NodeNo, Addr, GroupNo, MoveOwnership};

use crate::{node_map::LaunchId, socket::Socket};

// Internal.

#[message]
pub(crate) struct HandleConnection {
    pub(crate) local: (GroupNo, String),
    pub(crate) remote: (NodeNo, GroupNo, String),
    pub(crate) socket: MoveOwnership<Socket>,
    /// Initial window size of every flow.
    pub(crate) initial_window: i32,
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
        /// Initial window size for every flow.
        pub(crate) initial_window: i32,
    }

    #[message]
    pub(crate) struct UpdateFlow {
        #[serde(with = "sendable_addr")]
        pub(crate) addr: Addr,
        pub(crate) window_delta: i32,
    }

    #[message]
    pub(crate) struct CloseFlow {
        #[serde(with = "sendable_addr")]
        pub(crate) addr: Addr,
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

// See docs of `Addr` for details why it's required.
mod sendable_addr {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    use super::*;

    pub(super) fn serialize<S: Serializer>(addr: &Addr, serializer: S) -> Result<S::Ok, S::Error> {
        debug_assert!(addr.is_remote());
        addr.into_bits().serialize(serializer)
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Addr, D::Error> {
        let bits = u64::deserialize(deserializer)?;
        let addr = Addr::from_bits(bits);
        debug_assert!(addr.is_remote());
        Ok(addr)
    }
}
