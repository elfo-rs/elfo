use std::{future::Future, mem, sync::Arc, time::Duration};

use advise_timer::NewTimerSetup;
use eyre::{bail, eyre, Result, WrapErr};
use futures::StreamExt;
use tracing::{debug, error, info, warn};

use elfo_core::{
    message, msg, scope, tracing::TraceId, AnyMessage, Envelope, Message, MoveOwnership,
    RestartPolicy, _priv::MessageKind, addr::GroupNo, messages::ConfigUpdated, stream::Stream,
    time::Delay, RestartParams, Topology, UnattachedSource,
};

use crate::{
    codec::format::{NetworkAddr, NetworkEnvelope, NetworkEnvelopePayload},
    config::{self, Transport},
    connman::{self, log_conn, ConnMan, ConnectTransport, EstablishDecision, ManagedConnRef},
    node_map::{NodeInfo, NodeMap},
    protocol::{internode, ConnId, ConnectionFailed, ConnectionRole, GroupInfo, HandleConnection},
    socket::{self, ReadError, Socket},
    NetworkContext,
};

use tokio::time::Instant;

use self::{advise_timer::AdviseTimer, diff::Diff};

mod advise_timer;
mod diff;

/// Initial window size of every flow.
/// TODO: should be different for groups and actors.
const INITIAL_WINDOW_SIZE: i32 = 100_000;

#[message]
struct OpenConnectionsTick;

#[message]
enum ConnectionAllocation {
    NonAllocated { transport: ConnectTransport },
    Allocated { id: ConnId },
}

#[message]
struct ConnectionEstablished {
    socket: MoveOwnership<Socket>,
    connection: ConnectionAllocation,
}

#[message]
struct ConnectionAccepted {
    socket: MoveOwnership<Socket>,
    remote_msg: RemoteSwitchMessage,
    id: ConnId,
}

#[message(part)]
enum RemoteSwitchMessage {
    Control(internode::SwitchToControl),
    Data(internode::SwitchToData),
}

pub(super) struct Discovery {
    cfg: config::Config,
    ctx: NetworkContext,
    node_map: Arc<NodeMap>,
    connman: ConnMan,
    advise_timer: AdviseTimer,
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
        let node_map = Arc::new(NodeMap::new(&topology));
        Self {
            connman: ConnMan::new(
                connman::Config {
                    reconnect_interval: cfg.discovery.attempt_interval,
                },
                Arc::clone(&node_map),
            ),
            node_map,
            advise_timer: AdviseTimer::new(),
            cfg,
            ctx,
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
                OpenConnectionsTick => {
                    self.open_connections();
                }

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
                msg @ ConnectionEstablished => self.on_connection_established(msg),
                msg @ ConnectionAccepted => self.on_connection_accepted(msg),
                msg @ ConnectionFailed => self.on_connection_failed(msg),
            });
        }

        Ok(())
    }

    fn open_connections(&mut self) {
        let (next_check_advise, connections) = self.connman.open_connections();
        if let Some(advise) = next_check_advise {
            match self.advise_timer.feed(advise) {
                NewTimerSetup::Do { at } => {
                    self.ctx.attach(Delay::until(at, OpenConnectionsTick));
                    info!(
                        after = ?advise.duration_since(Instant::now()),
                        "scheduled next connection opening",
                    );
                }
                NewTimerSetup::OldIsStillTicking => {}
            }
        }

        let capabilities = self.get_capabilities();
        for (prev_state, id) in connections {
            let conn = self.connman.get(id).unwrap();
            log_conn!(
                info,
                conn,
                "opening connection",
                prev_state = prev_state,
                addr = conn.transport().display(),
            );

            let stream = Self::open_existing_connection(capabilities, &self.node_map, conn);
            self.ctx.attach(stream);
        }
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

            for transport in new {
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
            let cloned = transport.clone();
            let stream = socket::listen(transport, node_no, launch_id, capabilities)
                .await
                .wrap_err_with(|| eyre!("cannot listen {transport}"))?
                .map(move |socket| ConnectionEstablished {
                    socket: socket.into(),
                    connection: ConnectionAllocation::NonAllocated {
                        transport: ConnectTransport::incoming(cloned.clone()),
                    },
                });

            info!(message = "listening for connections", addr = %transport);

            self.ctx.attach(Stream::from_futures03(stream));
        }

        Ok(())
    }

    fn discover_all(&mut self) {
        // Nuh uh, borrowing.
        let mut idx = 0;
        while idx < self.cfg.discovery.predefined.len() {
            let transport = self.cfg.discovery.predefined[idx].clone();
            self.discover(transport);
            idx += 1;
        }
    }

    fn discover(&mut self, transport: Transport) {
        self.open_new_connection(transport, ConnectionRole::Control);
    }

    fn open_new_connection(&mut self, transport: Transport, role: ConnectionRole) {
        self.connman
            .insert_new(role, ConnectTransport::outgoing(transport));
        self.open_connections();
    }

    fn open_existing_connection(
        capabilities: socket::Capabilities,
        node_map: &NodeMap,
        conn: ManagedConnRef<'_>,
    ) -> UnattachedSource<Stream<Result<impl Message, impl Message>>> {
        let node_no = node_map.this.node_no;
        let launch_id = node_map.this.launch_id;

        let id = conn.id();
        let role = conn.role();
        let transport = conn.transport().clone();

        Stream::once(async move {
            debug!(
                message = "connecting to peer",
                addr = %transport.display(),
                role = %role,
                capabilities = %capabilities,
            );

            match socket::connect(&transport.transport, node_no, launch_id, capabilities).await {
                Ok(socket) => Ok(ConnectionEstablished {
                    socket: socket.into(),
                    connection: ConnectionAllocation::Allocated { id },
                }),
                Err(err) => {
                    // TODO: some errors should be logged as warnings.
                    info!(
                        message = "cannot connect",
                        error = %err,
                        addr = %transport.display(),
                        role = %role,
                    );

                    Err(ConnectionFailed { id })
                }
            }
        })
    }

    fn on_connection_established(&mut self, msg: ConnectionEstablished) {
        let ConnectionEstablished { socket, connection } = msg;
        let conn_id = match connection {
            ConnectionAllocation::Allocated { id } => id,
            ConnectionAllocation::NonAllocated { transport } => self
                .connman
                .insert_establishing(ConnectionRole::Unknown, transport),
        };

        let socket = socket.take().unwrap();
        let conn = match self
            .connman
            .on_connection_established(
                conn_id,
                connman::SocketInfo {
                    raw: socket.info.clone(),
                    peer: socket.peer,
                    capabilities: socket.capabilities,
                },
            )
            .unwrap()
        {
            EstablishDecision::Proceed(conn) => conn,
            EstablishDecision::Reject => return,
        };
        let role = conn.role();
        let id = conn.id();

        let node_map = self.node_map.clone();
        let idle_timeout = self.cfg.idle_timeout;

        self.ctx.attach(Stream::once(async move {
            let info = socket.info.clone();
            let peer = socket.peer;

            accept_connection(socket, id, role, &node_map.this, idle_timeout)
                .await
                .map_err(|err| {
                    warn!(
                        message = "new connection rejected",
                        error = %format!("{err:#}"),
                        socket = %info,
                        peer = %peer,
                        role = %role,
                    );
                    ConnectionFailed { id }
                })
        }));
    }

    fn on_connection_accepted(&mut self, msg: ConnectionAccepted) {
        let socket = msg.socket.take().unwrap();
        let id = msg.id;

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

                self.control_maintenance(socket, id);

                let conn = self
                    .connman
                    .on_connection_accepted(id, ConnectionRole::Control)
                    .unwrap();

                // Only initiator (client) can start new connections,
                // because he knows the transport address.
                let transport = conn.transport();
                if !transport.direction.is_outgoing() {
                    return;
                }

                let this_node = &self.node_map.clone().this;
                let transport = transport.transport.clone();

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
                        self.open_new_connection(transport.clone(), role);
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

                self.connman
                    .on_connection_accepted(
                        id,
                        ConnectionRole::Data {
                            local_group_no,
                            remote_group_no,
                        },
                    )
                    .unwrap();
                let failed = ConnectionFailed { id };

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

                            _ = self.ctx.unbounded_send_to(self.ctx.addr(), failed);
                            return;
                        }
                    };

                let res = self.ctx.try_send_to(
                    self.ctx.group(),
                    HandleConnection {
                        id,
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
        let ConnectionFailed { id } = msg;
        self.connman.on_connection_failed(id).unwrap();
        self.open_connections();
    }

    fn control_maintenance(&mut self, mut socket: Socket, id: ConnId) {
        self.ctx.attach(Stream::once(async move {
            let err = control_maintenance(&mut socket).await.unwrap_err();

            info!(
                message = "control connection closed",
                socket = %socket.info,
                peer = %socket.peer,
                reason = format!("{err:#}"), // TODO: use `AsRef<dyn Error>`
            );

            ConnectionFailed { id }
        }));
    }
}

async fn accept_connection(
    mut socket: Socket,
    id: ConnId,
    role: ConnectionRole,
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
        id,
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
