use std::{future::Future, mem, sync::Arc, time::Duration};

use eyre::{bail, eyre, Result, WrapErr};
use futures::StreamExt;
use tracing::{debug, error, info, warn};

use elfo_core::{
    message, msg, scope, tracing::TraceId, AnyMessage, Envelope, Message, MoveOwnership,
    RestartPolicy, _priv::MessageKind, addr::GroupNo, messages::ConfigUpdated, stream::Stream,
    time::Delay, RestartParams, Topology,
};

use crate::{
    codec::format::{NetworkAddr, NetworkEnvelope, NetworkEnvelopePayload},
    config::{self, Transport},
    node_map::{NodeInfo, NodeMap},
    protocol::{internode, ConnectionFailed, ConnectionRole, GroupInfo, HandleConnection},
    socket::{self, ReadError, Socket},
    NetworkContext,
};

use self::diff::Diff;

mod diff;

/// Initial window size of every flow.
/// TODO: should be different for groups and actors.
const INITIAL_WINDOW_SIZE: i32 = 100_000;

#[message]
struct ConnectionEstablished {
    socket: MoveOwnership<Socket>,
    role: ConnectionRole,
    // `Some` if this node is initiator.
    transport: Option<Transport>,
}

#[message]
struct ConnectionAccepted {
    socket: MoveOwnership<Socket>,
    remote_msg: RemoteSwitchMessage,
    // `Some` only on the client side.
    transport: Option<Transport>,
}

#[message(part)]
enum RemoteSwitchMessage {
    Control(internode::SwitchToControl),
    Data(internode::SwitchToData),
}

impl RemoteSwitchMessage {
    fn role(&self) -> ConnectionRole {
        match self {
            RemoteSwitchMessage::Control(_) => ConnectionRole::Control,
            RemoteSwitchMessage::Data(msg) => ConnectionRole::Data {
                local_group_no: msg.your_group_no,
                remote_group_no: msg.my_group_no,
            },
        }
    }
}

#[message]
struct OpenConnection {
    role: ConnectionRole,
    transport: Transport,
}

pub(super) struct Discovery {
    cfg: config::Config,
    ctx: NetworkContext,
    node_map: Arc<NodeMap>,
}

// TODO: move control connections to dedicated actors.
// TODO: detect duplicate nodes.
// TODO: discover tick.
// TODO: status of in-progress connections
// TODO: launch_id changed.
// TODO: graceful termination.

impl Discovery {
    pub(super) fn new(ctx: NetworkContext, topology: Topology) -> Self {
        let cfg = ctx.config().clone();
        Self {
            cfg,
            ctx,
            node_map: Arc::new(NodeMap::new(&topology)),
        }
    }

    pub(super) async fn main(mut self) -> Result<()> {
        // The default restart policy of this group is `never`, so override it.
        self.ctx
            .set_restart_policy(RestartPolicy::on_failure(RestartParams::new(
                Duration::from_secs(5),
                Duration::from_secs(30),
            )));

        self.listen().await?;
        self.discover_all();

        while let Some(envelope) = self.ctx.recv().await {
            msg!(match envelope {
                ConfigUpdated => self.on_update_config(),

                // Connection management.
                //
                //   ┌──────────┐ handshake ┌─────────────┐
                //   │ Unopened ├───────────► Established │
                //   └───▲────┬─┘           └──┬───┬──────┘
                //       │    │                │   │ switch to
                //  delay│    │   ┌────────────┘   │ data/control
                //    ┌──┴────▼┐  │          ┌─────▼────┐
                //    │ Failed ◄──┴──────────┤ Accepted │
                //    └────────┘             └──────────┘
                // ~
                msg @ OpenConnection => self.on_open_connection(msg),
                msg @ ConnectionEstablished => self.on_connection_established(msg),
                msg @ ConnectionAccepted => self.on_connection_accepted(msg),
                msg @ ConnectionFailed => self.on_connection_failed(msg),
            });
        }

        Ok(())
    }

    fn get_compression(&self) -> socket::Compression {
        use socket::Algorithms as Algos;

        let mut compression = socket::Compression::empty();
        compression.toggle(Algos::LZ4, self.cfg.compression.lz4);
        compression
    }

    fn get_capabilities(&self) -> socket::Capabilities {
        let compression = self.get_compression();

        socket::Capabilities::new(compression)
    }

    fn on_update_config(&mut self) {
        // TODO: Update listeners.
        let cfg = self.ctx.config().clone();
        let old = mem::replace(&mut self.cfg, cfg);

        self.update_discovery(old.discovery);
    }

    fn update_discovery(&mut self, old: config::DiscoveryConfig) {
        let config::DiscoveryConfig {
            predefined,
            attempt_interval: _,
        } = old;

        {
            let Diff { new, removed } = Diff::make(
                predefined.into_iter().collect(),
                &self.ctx.config().discovery.predefined,
            );

            for transport in &new {
                self.discover(transport);
            }

            // FIXME: handle removal.
            if !removed.is_empty() {
                warn!(
                    ?removed,
                    "got removal of several discovery.predefined entries, this is not supported as of now"
                );
            }
        }
    }

    async fn listen(&mut self) -> Result<()> {
        let node_no = self.node_map.this.node_no;
        let launch_id = self.node_map.this.launch_id;
        let capabilities = self.get_capabilities();

        for transport in &self.cfg.listen {
            let stream = socket::listen(transport, node_no, launch_id, capabilities)
                .await
                .wrap_err_with(|| eyre!("cannot listen {transport}"))?
                .map(|socket| ConnectionEstablished {
                    role: ConnectionRole::Unknown,
                    socket: socket.into(),
                    transport: None,
                });

            info!(message = "listening for connections", addr = %transport);

            self.ctx.attach(Stream::from_futures03(stream));
        }

        Ok(())
    }

    fn discover_all(&mut self) {
        for transport in &self.cfg.discovery.predefined {
            self.discover(transport);
        }
    }

    fn discover(&self, transport: &Transport) {
        self.open_connection(transport, ConnectionRole::Control);
    }

    fn open_connection(&self, transport: &Transport, role: ConnectionRole) {
        let _ = self.ctx.unbounded_send_to(
            self.ctx.addr(),
            OpenConnection {
                role,
                transport: transport.clone(),
            },
        );
    }

    fn on_open_connection(&mut self, msg: OpenConnection) {
        let OpenConnection { transport, role } = msg;
        let node_no = self.node_map.this.node_no;
        let launch_id = self.node_map.this.launch_id;
        let capabilities = self.get_capabilities();

        self.ctx.attach(Stream::once(async move {
            debug!(
                message = "connecting to peer",
                addr = %transport,
                role = %role,
                capabilities = %capabilities,
            );

            match socket::connect(&transport, node_no, launch_id, capabilities).await {
                Ok(socket) => Ok(ConnectionEstablished {
                    socket: socket.into(),
                    role,
                    transport: Some(transport),
                }),
                Err(err) => {
                    // TODO: some errors should be logged as warnings.
                    info!(
                        message = "cannot connect",
                        error = %err,
                        addr = %transport,
                        role = %role,
                    );

                    Err(ConnectionFailed {
                        role,
                        transport: Some(transport),
                    })
                }
            }
        }));
    }

    fn on_connection_established(&mut self, msg: ConnectionEstablished) {
        let socket = msg.socket.take().unwrap();

        debug!(
            message = "new connection established",
            socket = %socket.info,
            peer = %socket.peer,
            role = %msg.role,
            capabilities = %socket.capabilities,
        );

        if socket.peer.node_no == self.node_map.this.node_no {
            info!(
                message = "connection to self ignored",
                socket = %socket.info,
                peer = %socket.peer,
                role = %msg.role,
            );
            return;
        }

        let node_map = self.node_map.clone();
        let idle_timeout = self.cfg.idle_timeout;

        self.ctx.attach(Stream::once(async move {
            let info = socket.info.clone();
            let peer = socket.peer.clone();

            accept_connection(
                socket,
                msg.role.clone(),
                msg.transport.clone(),
                &node_map.this,
                idle_timeout,
            )
            .await
            .map_err(|err| {
                warn!(
                    message = "new connection rejected",
                    error = %format!("{err:#}"),
                    socket = %info,
                    peer = %peer,
                    role = %msg.role,
                );
                ConnectionFailed {
                    role: msg.role,
                    transport: msg.transport,
                }
            })
        }));
    }

    fn on_connection_accepted(&mut self, msg: ConnectionAccepted) {
        let socket = msg.socket.take().unwrap();
        let role = msg.remote_msg.role();

        info!(
            message = "new connection accepted",
            socket = %socket.info,
            peer = %socket.peer,
            role = %role,
        );

        match msg.remote_msg {
            RemoteSwitchMessage::Control(remote) => {
                {
                    let mut nodes = self.node_map.nodes.lock();
                    nodes.insert(
                        socket.peer.node_no,
                        NodeInfo {
                            node_no: socket.peer.node_no,
                            launch_id: socket.peer.launch_id,
                            groups: remote.groups.clone(),
                        },
                    );

                    // TODO: check launch_id.
                }

                self.control_maintenance(socket, msg.transport.clone());

                // Only initiator (client) can start new connections,
                // because he knows the transport address.
                let Some(transport) = msg.transport else {
                    return;
                };

                let this_node = &self.node_map.clone().this;

                // Open connections for all interesting pairs of groups.
                infer_connections(&remote.groups, &this_node.groups)
                    .map(|(remote_group_no, local_group_no)| (local_group_no, remote_group_no))
                    .chain(infer_connections(&this_node.groups, &remote.groups))
                    .collect::<Vec<_>>()
                    .into_iter()
                    .for_each(|(local_group_no, remote_group_no)| {
                        let role = ConnectionRole::Data {
                            local_group_no,
                            remote_group_no,
                        };

                        // TODO: save stream to cancel later.
                        // TODO: connect without DNS resolving here.
                        self.open_connection(&transport, role);
                    });
            }
            RemoteSwitchMessage::Data(remote) => {
                let local_group_no = remote.your_group_no;
                let local_group_name = self
                    .node_map
                    .this
                    .groups
                    .iter()
                    .find(|g| g.group_no == local_group_no)
                    .map(|g| g.name.clone())
                    .ok_or("group not found");

                let remote_group_no = remote.my_group_no;
                let remote_group_name = self
                    .node_map
                    .nodes
                    .lock()
                    .get(&socket.peer.node_no)
                    .ok_or("node not found")
                    .and_then(|n| {
                        n.groups
                            .iter()
                            .find(|g| g.group_no == remote_group_no)
                            .map(|g| g.name.clone())
                            .ok_or("group not found")
                    });

                let failed = ConnectionFailed {
                    role,
                    transport: msg.transport.clone(),
                };

                let (local_group_name, remote_group_name) =
                    match (local_group_name, remote_group_name) {
                        (Ok(local_group_name), Ok(remote_group_name)) => {
                            (local_group_name, remote_group_name)
                        }
                        (local_group, remote_group) => {
                            // TODO: it should be error once connection manager is implemented.
                            info!(
                                message = "control and data connections contradict each other",
                                socket = %socket.info,
                                peer = %socket.peer,
                                ?local_group,
                                ?remote_group,
                            );

                            let _ = self.ctx.unbounded_send_to(self.ctx.addr(), failed);
                            return;
                        }
                    };

                let res = self.ctx.try_send_to(
                    self.ctx.group(),
                    HandleConnection {
                        local: GroupInfo {
                            node_no: self.node_map.this.node_no,
                            group_no: remote.your_group_no,
                            group_name: local_group_name,
                        },
                        remote: GroupInfo {
                            node_no: socket.peer.node_no,
                            group_no: remote.my_group_no,
                            group_name: remote_group_name,
                        },
                        transport: msg.transport.clone(),
                        socket: socket.into(),
                        initial_window: remote.initial_window,
                    },
                );

                if let Err(err) = res {
                    error!(message = "cannot start connection handler", error = %err);
                    let _ = self.ctx.unbounded_send_to(self.ctx.addr(), failed);
                }
            }
        }
    }

    fn on_connection_failed(&mut self, msg: ConnectionFailed) {
        let ConnectionFailed { role, transport } = msg;

        let Some(transport) = transport else {
            debug!(
                message = "connection failed, address to reconnect is unknown",
                role = %role,
            );
            return;
        };

        // TODO: autoreset interval, exponential backoff.
        let interval = self.cfg.discovery.attempt_interval;

        debug!(
            message = "connection failed, reconnecting after delay",
            addr = %transport,
            role = %role,
            delay = ?interval,
        );

        let msg = OpenConnection { role, transport };
        self.ctx.attach(Delay::new(interval, msg));
    }

    fn control_maintenance(&mut self, mut socket: Socket, transport: Option<Transport>) {
        self.ctx.attach(Stream::once(async move {
            let err = control_maintenance(&mut socket).await.unwrap_err();

            info!(
                message = "control connection closed",
                socket = %socket.info,
                peer = %socket.peer,
                reason = format!("{err:#}"), // TODO: use `AsRef<dyn Error>`
            );

            let role = ConnectionRole::Control;
            ConnectionFailed { role, transport }
        }));
    }
}

async fn accept_connection(
    mut socket: Socket,
    role: ConnectionRole,
    transport: Option<Transport>,
    this_node: &NodeInfo,
    idle_timeout: Duration,
) -> Result<ConnectionAccepted> {
    let remote_msg = match role {
        ConnectionRole::Unknown => {
            msg!(match recv(&mut socket, idle_timeout).await? {
                msg @ internode::SwitchToControl => {
                    let my_msg = internode::SwitchToControl {
                        groups: this_node.groups.clone(),
                    };
                    send_regular(&mut socket, idle_timeout, my_msg).await?;
                    RemoteSwitchMessage::Control(msg)
                }
                msg @ internode::SwitchToData => {
                    let my_msg = internode::SwitchToData {
                        my_group_no: msg.your_group_no,
                        your_group_no: msg.my_group_no,
                        initial_window: INITIAL_WINDOW_SIZE,
                    };
                    send_regular(&mut socket, idle_timeout, my_msg).await?;
                    RemoteSwitchMessage::Data(msg)
                }
                envelope =>
                    return Err(unexpected_message_error(
                        envelope,
                        &["SwitchToControl", "SwitchToData"]
                    )),
            })
        }
        ConnectionRole::Control => {
            let my_msg = internode::SwitchToControl {
                groups: this_node.groups.clone(),
            };
            send_regular(&mut socket, idle_timeout, my_msg).await?;
            let msg = recv_regular::<internode::SwitchToControl>(&mut socket, idle_timeout).await?;
            RemoteSwitchMessage::Control(msg)
        }
        ConnectionRole::Data {
            local_group_no,
            remote_group_no,
        } => {
            let my_msg = internode::SwitchToData {
                my_group_no: local_group_no,
                your_group_no: remote_group_no,
                initial_window: INITIAL_WINDOW_SIZE,
            };
            send_regular(&mut socket, idle_timeout, my_msg).await?;
            let msg = recv_regular::<internode::SwitchToData>(&mut socket, idle_timeout).await?;
            RemoteSwitchMessage::Data(msg)
        }
    };

    Ok(ConnectionAccepted {
        remote_msg,
        socket: socket.into(),
        transport,
    })
}

async fn control_maintenance(socket: &mut Socket) -> Result<()> {
    // TODO: we should use these values from the config.
    // However, prior to it, this code should be rewritten to split logic of sending
    // pings and responding to pings. So, for now, we use hardcoded large
    // values. It detects dead connections, but not configurable.
    // Also, we should reuse `IdleTracker` (used in data connections) here.
    let ping_interval = Duration::from_secs(10);
    let idle_timeout = Duration::from_secs(120);

    let mut interval = tokio::time::interval(ping_interval);

    loop {
        interval.tick().await;
        scope::set_trace_id(TraceId::generate());
        send_regular(socket, idle_timeout, internode::Ping { payload: 0 }).await?;
        recv_regular::<internode::Ping>(socket, idle_timeout).await?;
        send_regular(socket, idle_timeout, internode::Pong { payload: 0 }).await?;
        recv_regular::<internode::Pong>(socket, idle_timeout).await?;
    }
}

fn infer_connections<'a>(
    one: &'a [internode::GroupInfo],
    two: &'a [internode::GroupInfo],
) -> impl Iterator<Item = (GroupNo, GroupNo)> + 'a {
    one.iter().flat_map(move |o| {
        two.iter()
            .filter(move |t| o.interests.contains(&t.name))
            .map(move |t| (o.group_no, t.group_no))
    })
}

async fn send_regular<M: Message>(
    socket: &mut Socket,
    idle_timeout: Duration,
    message: M,
) -> Result<()> {
    let name = message.name();
    let envelope = NetworkEnvelope {
        sender: NetworkAddr::NULL,    // doesn't matter
        recipient: NetworkAddr::NULL, // doesn't matter
        trace_id: scope::trace_id(),
        payload: NetworkEnvelopePayload::Regular {
            message: AnyMessage::new(message),
        },
    };

    let send_future = socket.write.send(&envelope);
    timeout(idle_timeout, send_future)
        .await
        .wrap_err_with(|| eyre!("cannot send {}", name))
}

async fn recv(socket: &mut Socket, idle_timeout: Duration) -> Result<Envelope> {
    let receiving = async {
        socket.read.recv().await.map_err(|err| match err {
            ReadError::EnvelopeSkipped(..) => eyre!("failed to decode message"),
            ReadError::Fatal(report) => report,
        })
    };

    let envelope = timeout(idle_timeout, receiving)
        .await
        .wrap_err("cannot receive a message")?
        .ok_or_else(|| eyre!("connection closed by peer"))?;

    let message = match envelope.payload {
        NetworkEnvelopePayload::Regular { message } => message,
        _ => bail!("unexpected message kind"),
    };

    // TODO: should we skip changing here if it's an initiator?
    scope::set_trace_id(envelope.trace_id);

    Ok(Envelope::new(
        message,
        MessageKind::regular(envelope.sender.into_remote()),
    ))
}

async fn recv_regular<M: Message>(socket: &mut Socket, idle_timeout: Duration) -> Result<M> {
    msg!(match recv(socket, idle_timeout).await? {
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

async fn timeout<T>(duration: Duration, fut: impl Future<Output = Result<T>>) -> Result<T> {
    tokio::time::timeout(duration, fut).await?
}
