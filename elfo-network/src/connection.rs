use std::iter;

use fxhash::{FxHashMap, FxHashSet};
use tokio::time::Instant;
use tracing::{debug, warn};

use elfo_core::{
    message,
    node::{self, NodeNo},
    GroupNo,
};

use crate::{
    config::Transport,
    node_map::{LaunchId, NodeInfo},
    protocol::internode::GroupInfo,
    socket::Peer,
};

type ConnectionId = u32;

pub(super) struct ConnectionsTracker {
    /// The node number of the current node.
    node_no: NodeNo,
    /// All known nodes, including the current one.
    nodes: FxHashMap<NodeNo, NodeState>,
    /// All known transports for connecting.
    transports: FxHashMap<Transport, TransportStatus>,
}

// TODO: prevent overlapping connections before handshake
// TODO: should connections also have backoff to prevent buggy loops?

struct NodeState {
    launch_id: LaunchId,
    allowed_transports: FxHashSet<Transport>,
    forbidden_transports: FxHashSet<Transport>,
    connections: FxHashMap<ConnectionRole, ConnectionStatus>,
}

enum ConnectionStatus {
    Active,
    Failed {
        since: Instant,
        // TODO: exponential backoff
    },
    Terminated,
}

#[derive(Debug)]
enum TransportStatus {
    Unknown,
    Used,
    Blocked {
        since: Instant,
        // TODO: exponential backoff
    },
}

pub(super) struct Wish {
    pub(super) transport: Transport,
    pub(super) role: ConnectionRole,
}

#[derive(PartialEq, Eq, Hash)]
pub(super) enum ConnectionRole {
    Control,
    Data { local: GroupNo, remote: GroupNo },
}

impl ConnectionsTracker {
    pub(super) fn new(node_no: NodeNo, launch_id: LaunchId) -> Self {
        let this_node_state = NodeState {
            launch_id,
            allowed_transports: <_>::default(),
            forbidden_transports: <_>::default(),
            connections: <_>::default(),
        };

        Self {
            node_no,
            // Add the current node immediately.
            nodes: iter::once((node_no, this_node_state)).collect(),
            transports: <_>::default(),
        }
    }

    /// Updates known addresses and deletes mentions of unknown ones.
    pub(super) fn set_known_transports(&self, transports: &[Transport]) {
        // Update the list of known addresses.
        self.transports.clear();
        self.transports.extend(transports.iter().cloned());

        // Delete any mentions of unknown addresses.
        for node in self.nodes.values_mut() {
            node.allowed_transports
                .retain(|addr| self.transports.contains(addr));
            node.forbidden_transports
                .retain(|addr| self.transports.contains(addr));
        }
    }

    pub(super) fn block_transport(&mut self, transport: &Transport) {
        let Some(status) = self.transports.get_mut(transport) else {
            // Not warning because known transports can be updated right before blocking.
            debug!(message = "cannot block unknown transport", transport = %transport);
            return;
        };

        if !matches!(status, TransportStatus::Available) {
            debug!(
                message = "cannot block transport, it's already unavailable",
                transport = %transport,
                status  = ?status,
            );
            return;
        }

        // TODO
    }

    /// Returns `false` if a connection must be ignored.
    pub(super) fn on_handshake(&self, peer: &Peer) -> bool {
        // Firstly, prevent self-connections.
        if self.this.node_no == peer.node_no {
            if self.this.launch_id != peer.launch_id {
                // TODO
            }

            debug!(message = "ignored connection to self", peer = %peer.transport);
            return false;
        }

        // Secondly, detect launch_id mismatch.
        if let Some(state) = self.nodes.get(&peer.node_no) {
            if state.info.launch_id != peer.launch_id {
                // TODO
                return false;
            }
        }

        true
    }

    /// Returns `false` if a connection must be ignored.
    pub(super) fn register(&mut self, peer: &Peer, role: &ConnectionRole) -> bool {
        // Must always be called after `on_handshake`.
        assert_ne!(peer.node_no, self.this.node_no);
    }

    pub(super) fn unregister(&self, node_no: NodeNo, role: &ConnectionRole, terminated: bool) {}

    pub(super) fn wishes(&self) -> impl Iterator<Item = Wish> + '_ {
        let unknown = self
            .transports
            .iter()
            .filter(|(_, status)| matches!(status, TransportStatus::Unknown))
            .map(|(transport, _)| Wish {
                transport: transport.clone(),
                role: ConnectionRole::Control,
            });

        unknown
    }
}
