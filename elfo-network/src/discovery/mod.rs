use std::{future::Future, mem, sync::Arc, time::Duration};

use advise_timer::NewTimerSetup;
use eyre::{bail, eyre, Result, WrapErr};
use futures::StreamExt;
use slotmap::SecondaryMap;
use tracing::{error, info, warn};

use elfo_core::{
    message, msg, scope,
    stream::{Stream, StreamItem},
    tracing::TraceId,
    AnyMessage, Envelope, Message, MoveOwnership, RestartPolicy, SourceHandle, UnattachedSource,
    _priv::MessageKind,
    addr::GroupNo,
    messages::ConfigUpdated,
    time::Delay,
    RestartParams, Topology,
};
use elfo_utils::ward;

use crate::{
    codec::format::{NetworkAddr, NetworkEnvelope, NetworkEnvelopePayload},
    config::{self, Transport},
    connman::{self, log_conn, Command, ConnMan, ConnectTransport, EstablishDecision},
    node_map::{NodeInfo, NodeMap},
    protocol::{
        internode, AbortConnection, ConnId, ConnectionFailed, ConnectionRole, HandleConnection,
    },
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
struct ManageConnectionsTick;

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
    // Stream handles for connections driven by this actor.
    // It's used for all control connections and establishing data connections.
    streams: SecondaryMap<ConnId, Box<dyn SourceHandle + Send>>,
    advise_timer: AdviseTimer,
}

// TODO: detect duplicate nodes.
// TODO: launch_id changed.
// TODO: graceful termination.

impl Discovery {
    pub(super) fn new(ctx: NetworkContext, topology: Topology) -> Self {
        let cfg = ctx.config().clone();
        let node_map = Arc::new(NodeMap::new(&topology));
        Self {
            ctx,
            connman: ConnMan::new(
                connman::Config {
                    reconnect_interval: cfg.discovery.attempt_interval,
                },
                Arc::clone(&node_map),
            ),
            streams: <_>::default(),
            advise_timer: AdviseTimer::new(),
            node_map,
            cfg,
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
                ManageConnectionsTick => {
                    self.manage_connections();
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

    fn manage_connections(&mut self) {
        let (next_check_advise, commands) = self.connman.manage_connections();

        if let Some(advise) = next_check_advise {
            match self.advise_timer.feed(advise) {
                NewTimerSetup::Do { at } => {
                    self.ctx.attach(Delay::until(at, ManageConnectionsTick));
                    info!(
                        after = ?advise.duration_since(Instant::now()),
                        "scheduled next connection opening",
                    );
                }
                NewTimerSetup::OldIsStillTicking => {}
            }
        }

        for command in commands {
            match command {
                Command::Open(conn_id) => self.open_existing_connection(conn_id),
            }
        }
    }

    fn abort_connection(&mut self, conn_id: ConnId) {
        // Firstly, check if a connection is driven by this actor.
        if let Some(handle) = self.streams.remove(conn_id) {
            if handle.terminate_by_ref() {
                self.connman.on_connection_failed(conn_id);
                // A connection is driven by this actor, so we don't need to notify a worker.
                return;
            }
        }

        let conn = ward!(self.connman.get(conn_id));

        if let ConnectionRole::Data {
            local_group_no,
            remote_group_no,
        } = conn.role()
        {
            let remote_node_no = ward!(conn.socket_info()).peer.node_no;

            let local = ward!(self.node_map.local_group_meta(local_group_no));
            let remote = ward!(self
                .node_map
                .remote_group_meta(remote_node_no, remote_group_no));

            _ = self.ctx.unbounded_send_to(
                self.ctx.group(),
                AbortConnection {
                    id: conn_id,
                    local,
                    remote,
                },
            );
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
        // TODO: update listeners.
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

            for transport in removed {
                for conn_id in self.connman.abort_by_transport(&transport) {
                    self.abort_connection(conn_id);
                }
            }
        }

        self.manage_connections();
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
        self.manage_connections();
    }

    fn discover(&mut self, transport: Transport) {
        self.open_new_connection(transport, ConnectionRole::Control);
    }

    fn open_new_connection(&mut self, transport: Transport, role: ConnectionRole) {
        self.connman
            .insert_new(role, ConnectTransport::outgoing(transport));
    }

    fn open_existing_connection(&mut self, conn_id: ConnId) {
        let conn = ward!(self.connman.get(conn_id));
        let capabilities = self.get_capabilities();

        let node_no = self.node_map.this.node_no;
        let launch_id = self.node_map.this.launch_id;

        let id = conn.id();
        let role = conn.role();
        let transport = conn.transport().clone();

        log_conn!(
            debug,
            conn,
            "connecting to peer",
            addr = transport.display(),
        );

        let stream = Stream::once(async move {
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
        });
        self.attach_conn_stream(conn_id, stream);
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
            EstablishDecision::Proceed => &self.connman[conn_id],
            EstablishDecision::Reject => return,
        };
        let role = conn.role();
        let id = conn.id();

        let node_map = self.node_map.clone();
        let idle_timeout = self.cfg.idle_timeout;

        let stream = Stream::once(async move {
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
        });
        self.attach_conn_stream(conn_id, stream);
    }

    fn on_connection_accepted(&mut self, msg: ConnectionAccepted) {
        let socket = msg.socket.take().unwrap();
        let conn_id = msg.id;

        match msg.remote_msg {
            RemoteSwitchMessage::Control(params) => {
                {
                    let mut nodes = self.node_map.nodes.lock();
                    nodes.insert(
                        socket.peer.node_no,
                        NodeInfo {
                            node_no: socket.peer.node_no,
                            launch_id: socket.peer.launch_id,
                            groups: params.groups.clone(),
                        },
                    );

                    // TODO: check launch_id.
                }

                self.control_maintenance(socket, conn_id);

                self.connman
                    .on_connection_accepted(conn_id, ConnectionRole::Control);

                let conn = self.connman.get(conn_id).unwrap();

                // Only initiator (client) can start new connections,
                // because he knows the transport address.
                let transport = conn.transport();
                if !transport.direction.is_outgoing() {
                    return;
                }

                let this_node = &self.node_map.clone().this;
                let transport = transport.transport.clone();

                // Open connections for all interesting pairs of groups.
                infer_connections(&params.groups, &this_node.groups)
                    .map(|(remote_group_no, local_group_no)| (local_group_no, remote_group_no))
                    .chain(infer_connections(&this_node.groups, &params.groups))
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

                self.manage_connections();
            }
            RemoteSwitchMessage::Data(params) => {
                let local = self.node_map.local_group_meta(params.your_group_no);
                let remote = self
                    .node_map
                    .remote_group_meta(socket.peer.node_no, params.my_group_no);

                self.connman.on_connection_accepted(
                    conn_id,
                    ConnectionRole::Data {
                        local_group_no: params.your_group_no,
                        remote_group_no: params.my_group_no,
                    },
                );

                let (local, remote) = match (local, remote) {
                    (Some(local), Some(remote)) => (local, remote),
                    (local, remote) => {
                        // TODO: it should be error once connection manager is implemented.
                        info!(
                            message = "control and data connections contradict each other",
                            socket = %socket.info,
                            peer = %socket.peer,
                            local_group = ?local,
                            remote_group = ?remote,
                        );

                        self.connman.on_connection_failed(conn_id);
                        return;
                    }
                };

                let res = self.ctx.try_send_to(
                    self.ctx.group(),
                    HandleConnection {
                        id: conn_id,
                        local,
                        remote,
                        socket: socket.into(),
                        initial_window: params.initial_window,
                    },
                );

                if let Err(err) = res {
                    error!(message = "cannot start connection handler", error = %err);
                    self.connman.on_connection_failed(conn_id);
                }
            }
        }
    }

    fn on_connection_failed(&mut self, msg: ConnectionFailed) {
        let ConnectionFailed { id } = msg;
        self.connman.on_connection_failed(id);
        self.manage_connections();
    }

    fn control_maintenance(&mut self, mut socket: Socket, conn_id: ConnId) {
        let stream = Stream::once(async move {
            let err = control_maintenance(&mut socket).await.unwrap_err();

            info!(
                message = "control connection closed",
                socket = %socket.info,
                peer = %socket.peer,
                reason = format!("{err:#}"), // TODO: use `AsRef<dyn Error>`
            );

            ConnectionFailed { id: conn_id }
        });
        self.attach_conn_stream(conn_id, stream);
    }

    fn attach_conn_stream<M: StreamItem>(
        &mut self,
        conn_id: ConnId,
        stream: UnattachedSource<Stream<M>>,
    ) {
        let handle = self.ctx.attach(stream);
        if let Some(prev) = self.streams.insert(conn_id, Box::new(handle)) {
            debug_assert!(prev.is_terminated());
        }
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
