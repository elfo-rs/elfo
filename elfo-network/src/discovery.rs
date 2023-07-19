use std::sync::Arc;

use eyre::{bail, eyre, Result, WrapErr};
use futures::StreamExt;
use quanta::Instant;
use tracing::{debug, error, info, warn};

use elfo_core::{
    message, msg, node::NodeNo, scope, Addr, Envelope, Message, MoveOwnership, RestartPolicy,
    _priv::MessageKind, messages::ConfigUpdated, stream::Stream, GroupNo, Topology,
};

use crate::{
    codec::{NetworkEnvelope, NetworkEnvelopePayload},
    config::Transport,
    node_map::{LaunchId, NodeInfo, NodeMap},
    protocol::{
        internode::{self, GroupInfo},
        HandleConnection,
    },
    socket::{self, Socket},
    NetworkContext,
};

/// Initial window size of every flow.
/// TODO: should be different for groups and actors.
const INITIAL_WINDOW_SIZE: i32 = 100_000;

#[message]
struct ConnectionEstablished {
    role: ConnectionRole,
    socket: MoveOwnership<Socket>,
}

#[message(part)]
enum ConnectionRole {
    // Only possible if this node is a server.
    Unknown,
    Control(internode::SwitchToControl),
    Data(internode::SwitchToData),
}

impl ConnectionRole {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Unknown => "Unknown",
            Self::Control(_) => "Control",
            Self::Data(_) => "Data",
        }
    }
}

#[message]
struct ConnectionAccepted {
    is_initiator: bool,
    role: ConnectionRole,
    node_no: NodeNo,
    launch_id: LaunchId,
    socket: MoveOwnership<Socket>,
}

#[message]
struct ConnectionRejected {
    error: String,
    peer: Transport,
    retry: bool,
}

pub(super) struct Discovery {
    ctx: NetworkContext,
    node_map: Arc<NodeMap>,
}

// TODO: detect duplicate nodes.
// TODO: discover tick.
// TODO: status of in-progress connections
// TODO: launch_id changed.
// TODO: repeat discovery by timer.

impl Discovery {
    pub(super) fn new(ctx: NetworkContext, topology: Topology) -> Self {
        Self {
            ctx,
            node_map: Arc::new(NodeMap::new(&topology)),
        }
    }

    pub(super) async fn main(mut self) -> Result<()> {
        // The default restart policy of this group is `never`, so override it.
        self.ctx.set_restart_policy(RestartPolicy::on_failures());

        self.listen().await?;
        self.discover();

        while let Some(envelope) = self.ctx.recv().await {
            msg!(match envelope {
                ConfigUpdated => {
                    // TODO: update listeners.
                    // TODO: stop discovering for removed transports.
                    // TODO: self.discover();
                }
                msg @ ConnectionEstablished => self.on_connection_established(msg),
                msg @ ConnectionAccepted => self.on_connection_accepted(msg).await,
                msg @ ConnectionRejected => self.on_connection_rejected(msg),
            });
        }

        Ok(())
    }

    async fn listen(&mut self) -> Result<()> {
        for transport in self.ctx.config().listen.clone() {
            let stream = socket::listen(&transport)
                .await
                .wrap_err_with(|| eyre!("cannot listen {}", transport))?
                .map(|socket| ConnectionEstablished {
                    role: ConnectionRole::Unknown,
                    socket: socket.into(),
                });

            info!(
                message = "listening for connections",
                listener = %transport,
            );

            self.ctx.attach(Stream::from_futures03(stream));
        }

        Ok(())
    }

    fn discover(&mut self) {
        let msg = internode::SwitchToControl {
            groups: self.node_map.this.groups.clone(),
        };

        for transport in self.ctx.config().discovery.predefined.clone() {
            self.open_connection(&transport, ConnectionRole::Control(msg.clone()));
        }
    }

    fn open_connection(
        &mut self,
        peer: &Transport,
        role: ConnectionRole,
    ) -> Stream<ConnectionEstablished> {
        let interval = self.ctx.config().discovery.attempt_interval;
        let peer = peer.clone();

        self.ctx.attach(Stream::once(async move {
            loop {
                debug!(message = "connecting to peer", peer = %peer, role = ?role);

                match socket::connect(&peer).await {
                    Ok(socket) => {
                        break ConnectionEstablished {
                            role,
                            socket: socket.into(),
                        }
                    }
                    Err(err) => {
                        info!(message = "cannot connect", peer = %peer, error = %err);
                    }
                }

                // TODO: should we change trace_id?
                debug!(message = "retrying after some time", peer = %peer, delay = ?interval);
                tokio::time::sleep(interval).await;
            }
        }))
    }

    fn on_connection_established(&mut self, msg: ConnectionEstablished) {
        let socket = msg.socket.take().unwrap();

        info!(
            message = "new connection established",
            peer = %socket.peer,
            role = msg.role.as_str(),
        );

        let node_map = self.node_map.clone();
        self.ctx.attach(Stream::once(async move {
            let peer = socket.peer.clone();

            let result = accept_connection(socket, msg.role, &node_map.this).await;
            match result {
                Ok(Some(accepted)) => Ok(accepted),
                Ok(None) => Err(ConnectionRejected {
                    error: "node attempted to connect to itself".into(),
                    peer,
                    retry: false,
                }),
                Err(err) => {
                    let error_msg = err.to_string();
                    warn!(
                        message = "new connection rejected",
                        peer = %peer,
                        error = %error_msg,
                    );
                    Err(ConnectionRejected {
                        error: error_msg,
                        peer,
                        retry: true,
                    })
                }
            }
        }));
    }

    async fn on_connection_accepted(&mut self, msg: ConnectionAccepted) {
        let socket = msg.socket.take().unwrap();

        info!(
            message = "new connection accepted",
            peer = %socket.peer,
            peer_node_no = %msg.node_no,
            peer_launch_id = %msg.launch_id,
            role = msg.role.as_str(),
        );

        match msg.role {
            ConnectionRole::Unknown => unreachable!(),
            ConnectionRole::Control(remote) => {
                {
                    let mut nodes = self.node_map.nodes.lock();
                    nodes.insert(
                        msg.node_no,
                        NodeInfo {
                            node_no: msg.node_no,
                            launch_id: msg.launch_id,
                            groups: remote.groups.clone(),
                        },
                    );

                    // TODO: check launch_id.
                }

                // Only initiator (client) can start new connections,
                // because he knows the transport address.
                if !msg.is_initiator {
                    return;
                }

                let this_node = &self.node_map.clone().this;

                // Open connections for all interesting pairs of groups.
                infer_connections(&remote.groups, &this_node.groups)
                    .map(|(remote_group_no, local_group_no)| (local_group_no, remote_group_no))
                    .chain(infer_connections(&this_node.groups, &remote.groups))
                    .collect::<Vec<_>>()
                    .into_iter()
                    .for_each(|(local_group_no, remote_group_no)| {
                        // TODO: save stream to cancel later.
                        self.open_connection(
                            &socket.peer,
                            ConnectionRole::Data(internode::SwitchToData {
                                my_group_no: local_group_no,
                                your_group_no: remote_group_no,
                                initial_window: INITIAL_WINDOW_SIZE,
                            }),
                        );
                    });

                // TODO: start ping-pong process on the socket.
            }
            ConnectionRole::Data(remote) => {
                let local_group_name = self
                    .node_map
                    .this
                    .groups
                    .iter()
                    .find(|g| g.group_no == remote.your_group_no)
                    .map(|g| g.name.clone());

                let remote_group_name =
                    self.node_map.nodes.lock().get(&msg.node_no).and_then(|n| {
                        n.groups
                            .iter()
                            .find(|g| g.group_no == remote.my_group_no)
                            .map(|g| g.name.clone())
                    });

                let (local_group_name, remote_group_name) =
                    ward!(local_group_name.zip(remote_group_name), {
                        error!("control and data connections contradict each other");
                        return;
                    });

                let res = self.ctx.try_send_to(
                    self.ctx.group(),
                    HandleConnection {
                        local: (remote.your_group_no, local_group_name),
                        remote: (msg.node_no, remote.my_group_no, remote_group_name),
                        socket: socket.into(),
                        initial_window: remote.initial_window,
                    },
                );

                if let Err(err) = res {
                    error!(message = "cannot start connection handler", error = %err);
                    // TODO: something else?
                }
            }
        }
    }

    fn on_connection_rejected(&mut self, _msg: ConnectionRejected) {
        // TODO: something else? Retries?
    }
}

async fn accept_connection(
    mut socket: Socket,
    role: ConnectionRole,
    this_node: &NodeInfo,
) -> Result<Option<ConnectionAccepted>> {
    let start = Instant::now();
    let (node_no, launch_id) = handshake(&mut socket, this_node).await.map_err(|err| {
        debug!(
            message = "handshake failed",
            peer = %socket.peer,
            error = %err,
            elapsed = ?start.elapsed(),
        );
        err
    })?;

    debug!(
        message = "handshake succeeded",
        peer = %socket.peer,
        elapsed = ?start.elapsed(),
    );

    if node_no == this_node.node_no {
        info!(
            message = "connection to self ignored",
            peer = %socket.peer,
        );
        return Ok(None);
    }

    let (is_initiator, role) = match role {
        ConnectionRole::Unknown => {
            msg!(match recv(&mut socket).await? {
                msg @ internode::SwitchToControl => {
                    let my_msg = internode::SwitchToControl {
                        groups: this_node.groups.clone(),
                    };
                    send_regular(&mut socket, my_msg).await?;
                    (false, ConnectionRole::Control(msg))
                }
                msg @ internode::SwitchToData => {
                    let my_msg = internode::SwitchToData {
                        my_group_no: msg.your_group_no,
                        your_group_no: msg.my_group_no,
                        initial_window: INITIAL_WINDOW_SIZE,
                    };
                    send_regular(&mut socket, my_msg).await?;
                    (false, ConnectionRole::Data(msg))
                }
                envelope =>
                    return Err(unexpected_message_error(
                        envelope,
                        &["SwitchToControl", "SwitchToData"]
                    )),
            })
        }
        ConnectionRole::Control(msg) => {
            send_regular(&mut socket, msg).await?;
            let msg = recv_regular::<internode::SwitchToControl>(&mut socket).await?;
            (true, ConnectionRole::Control(msg))
        }
        ConnectionRole::Data(msg) => {
            send_regular(&mut socket, msg).await?;
            let msg = recv_regular::<internode::SwitchToData>(&mut socket).await?;
            (true, ConnectionRole::Data(msg))
        }
    };

    Ok(Some(ConnectionAccepted {
        is_initiator,
        role,
        node_no,
        launch_id,
        socket: socket.into(),
    }))
}

fn infer_connections<'a>(
    one: &'a [GroupInfo],
    two: &'a [GroupInfo],
) -> impl Iterator<Item = (GroupNo, GroupNo)> + 'a {
    one.iter().flat_map(move |o| {
        two.iter()
            .filter(move |t| o.interests.contains(&t.name))
            .map(move |t| (o.group_no, t.group_no))
    })
}

async fn handshake(socket: &mut Socket, this_node: &NodeInfo) -> Result<(NodeNo, LaunchId)> {
    let msg = internode::Handshake {
        node_no: this_node.node_no,
        launch_id: this_node.launch_id,
    };

    send_regular(socket, msg).await?;
    recv_regular::<internode::Handshake>(socket)
        .await
        .map(|msg| (msg.node_no, msg.launch_id))
}

async fn send_regular<M: Message>(socket: &mut Socket, msg: M) -> Result<()> {
    let name = msg.name();
    let envelope = NetworkEnvelope {
        sender: Addr::NULL,    // doesn't matter
        recipient: Addr::NULL, // doesn't matter
        trace_id: scope::trace_id(),
        payload: NetworkEnvelopePayload::Regular {
            message: msg.upcast(),
        },
    };

    socket
        .write
        .send(envelope)
        .await
        .wrap_err_with(|| eyre!("cannot send {}", name))
}

async fn recv(socket: &mut Socket) -> Result<Envelope> {
    let envelope = socket
        .read
        .recv()
        .await
        .wrap_err("cannot receive a message")?
        .ok_or_else(|| eyre!("connection closed before receiving any messages"))?;

    let message = match envelope.payload {
        NetworkEnvelopePayload::Regular { message } => message,
        _ => bail!("unexpected message kind"),
    };

    // TODO: should we skip changing here if it's an initiator?
    scope::set_trace_id(envelope.trace_id);

    Ok(Envelope::new(
        message,
        MessageKind::Regular {
            sender: envelope.sender,
        },
    ))
}

async fn recv_regular<M: Message>(socket: &mut Socket) -> Result<M> {
    msg!(match recv(socket).await? {
        msg @ M => Ok(msg),
        envelope => Err(unexpected_message_error(
            envelope,
            &[&elfo_core::dumping::extract_name_by_type::<M>().to_string()]
        )),
    })
}

fn unexpected_message_error(envelope: Envelope, expected: &[&str]) -> eyre::Report {
    eyre!(
        "unexpected message: {}, expected: {}",
        envelope.message().name(),
        expected.join(" or "),
    )
}
