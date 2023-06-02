use std::sync::Arc;

use eyre::{eyre, Result, WrapErr};
use futures::StreamExt;
use quanta::Instant;
use tracing::{debug, error, info, warn};

use elfo_core::{
    message, msg, node::NodeNo, scope, Addr, Context, Envelope, Message, MoveOwnership,
    _priv::MessageKind, messages::ConfigUpdated, stream::Stream,
};

use crate::{
    actors::Key,
    config::{Config, Transport},
    connection::{self, Connection},
    node_map::{LaunchId, NodeInfo, NodeMap},
    protocol::{internode, StartReceiver, StartTransmitter},
};

#[message]
struct ConnectionEstablished {
    role: ConnectionRole,
    connection: MoveOwnership<Connection>,
}

#[message(part)]
enum ConnectionRole {
    // Only possible if this node is a server.
    Unknown,
    Control(internode::NodeCompositionReport),
    Receiver(internode::ReadyToReceive),
    Transmitter(internode::ReadyToTransmit),
}

#[message]
struct ConnectionAccepted {
    is_initiator: bool,
    role: ConnectionRole,
    node_no: NodeNo,
    launch_id: LaunchId,
    connection: MoveOwnership<Connection>,
}

#[message]
struct ConnectionRejected {
    error: String,
    peer: Transport,
}

pub(super) struct Discovery {
    ctx: Context<Config, Key>,
    node_map: Arc<NodeMap>,
}

// TODO: detect duplicate nodes.
// TODO: discover tick.
// TODO: status of in-progress connections
// TODO: launch_id changed.
// TODO: repeat discovery by timer.

impl Discovery {
    pub(super) fn new(ctx: Context<Config, Key>, node_map: Arc<NodeMap>) -> Self {
        Self { ctx, node_map }
    }

    pub(super) async fn main(mut self) -> Result<()> {
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
            let stream = connection::listen(&transport)
                .await
                .wrap_err_with(|| eyre!("cannot listen {}", transport))?
                .map(|conn| ConnectionEstablished {
                    role: ConnectionRole::Unknown,
                    connection: conn.into(),
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
        let node_report = internode::NodeCompositionReport {
            groups: self.node_map.this.groups.clone(),
            interests: self.node_map.this.interests.clone(),
        };

        for transport in self.ctx.config().discovery.predefined.clone() {
            self.open_connection(&transport, ConnectionRole::Control(node_report.clone()));
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

                match connection::connect(&peer).await {
                    Ok(connection) => {
                        break ConnectionEstablished {
                            role,
                            connection: connection.into(),
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
        let connection = msg.connection.take().unwrap();

        info!(
            message = "new connection established",
            peer = %connection.peer,
            role = ?msg.role,
        );

        let node_map = self.node_map.clone();
        self.ctx.attach(Stream::once(async move {
            let peer = connection.peer.clone();

            accept_connection(connection, msg.role, &node_map.this)
                .await
                .map_err(|err| ConnectionRejected {
                    error: err.to_string(),
                    peer,
                })
        }));
    }

    async fn on_connection_accepted(&mut self, msg: ConnectionAccepted) {
        let mut connection = msg.connection.take().unwrap();

        info!(
            message = "new connection accepted",
            peer = %connection.peer,
            role = ?msg.role,
        );

        match msg.role {
            ConnectionRole::Unknown => unreachable!(),
            ConnectionRole::Control(remote_node) => {
                // TODO: should we update `NodeMap`?

                // The stream is empty, so it shouldn't block the actor.
                if let Err(err) = connection
                    .write
                    .send_flush(
                        make_regular(internode::NodeCompositionReport {
                            groups: self.node_map.this.groups.clone(),
                            interests: self.node_map.this.interests.clone(),
                        }),
                        Addr::NULL,
                    )
                    .await
                {
                    // TODO: another way?
                    return self.on_connection_rejected(ConnectionRejected {
                        error: format!("once accepted: {err}"),
                        peer: connection.peer,
                    });
                }

                if msg.is_initiator {
                    let this_node = &self.node_map.clone().this;

                    for group in remote_node
                        .groups
                        .iter()
                        .filter(|g| this_node.interests.contains(&g.name))
                    {
                        // TODO: save stream to cancel later.
                        self.open_connection(
                            &connection.peer,
                            ConnectionRole::Transmitter(internode::ReadyToTransmit {
                                group_no: group.group_no,
                            }),
                        );
                    }

                    for group in this_node
                        .groups
                        .iter()
                        .filter(|g| remote_node.interests.contains(&g.name))
                    {
                        // TODO: save stream to cancel later.
                        self.open_connection(
                            &connection.peer,
                            ConnectionRole::Receiver(internode::ReadyToReceive {
                                group_no: group.group_no,
                            }),
                        );
                    }
                }
            }
            ConnectionRole::Receiver(details) => {
                // The stream is empty, so it shouldn't block the actor.
                if let Err(err) = connection
                    .write
                    .send_flush(
                        make_regular(internode::ReadyToReceive {
                            group_no: details.group_no,
                        }),
                        Addr::NULL,
                    )
                    .await
                {
                    // TODO: another way?
                    return self.on_connection_rejected(ConnectionRejected {
                        error: format!("once accepted: {err}"),
                        peer: connection.peer,
                    });
                }

                let res = self.ctx.try_send_to(
                    self.ctx.group(),
                    StartReceiver {
                        node_no: msg.node_no,
                        local_group_no: details.group_no,
                        connection: connection.into(),
                    },
                );

                if let Err(err) = res {
                    error!(message = "cannot start receiver", error = %err);
                    // TODO: something else?
                }
            }
            ConnectionRole::Transmitter(details) => {
                // The stream is empty, so it shouldn't block the actor.
                if let Err(err) = connection
                    .write
                    .send_flush(
                        make_regular(internode::ReadyToTransmit {
                            group_no: details.group_no,
                        }),
                        Addr::NULL,
                    )
                    .await
                {
                    // TODO: another way?
                    return self.on_connection_rejected(ConnectionRejected {
                        error: format!("once accepted: {err}"),
                        peer: connection.peer,
                    });
                }

                let res = self.ctx.try_send_to(
                    self.ctx.group(),
                    StartTransmitter {
                        node_no: msg.node_no,
                        remote_group_no: details.group_no,
                        connection: connection.into(),
                    },
                );

                if let Err(err) = res {
                    error!(message = "cannot start transmitter", error = %err);
                    // TODO: something else?
                }
            }
        }
    }

    fn on_connection_rejected(&mut self, msg: ConnectionRejected) {
        // TODO: something else? Retries?
        warn!(
            message = "new connection rejected",
            peer = %msg.peer,
            error = %msg.error,
        );
    }
}

async fn accept_connection(
    mut connection: Connection,
    mut role: ConnectionRole,
    this_node: &NodeInfo,
) -> Result<ConnectionAccepted> {
    let start = Instant::now();
    let (node_no, launch_id) = handshake(&mut connection, this_node).await.map_err(|err| {
        debug!(
            message = "handshake failed",
            peer = %connection.peer,
            error = %err,
            elapsed = ?start.elapsed(),
        );
        err
    })?;

    debug!(
        message = "handshake succeeded",
        peer = %connection.peer,
        elapsed = ?start.elapsed(),
    );

    let is_initiator = !matches!(role, ConnectionRole::Unknown);
    if !is_initiator {
        let (envelope, _) = connection
            .read
            .recv()
            .await
            .wrap_err("receiving first message after handshake")?
            .ok_or_else(|| eyre!("connection closed after handshake"))?;

        role = msg!(match envelope {
            msg @ internode::NodeCompositionReport => ConnectionRole::Control(msg),
            msg @ internode::ReadyToTransmit =>
                ConnectionRole::Receiver(internode::ReadyToReceive {
                    group_no: msg.group_no,
                }),
            msg @ internode::ReadyToReceive =>
                ConnectionRole::Transmitter(internode::ReadyToTransmit {
                    group_no: msg.group_no,
                }),
            envelope => return Err(unexpected_message_error(envelope)),
        });
    }

    Ok(ConnectionAccepted {
        is_initiator,
        role,
        node_no,
        launch_id,
        connection: connection.into(),
    })
}

async fn handshake(
    connection: &mut Connection,
    this_node: &NodeInfo,
) -> Result<(NodeNo, LaunchId)> {
    let msg = internode::Handshake {
        node_no: this_node.node_no,
        launch_id: this_node.launch_id,
    };

    connection
        .write
        .send_flush(make_regular(msg), Addr::NULL)
        .await
        .wrap_err("sending Handshake")?;

    let (envelope, _) = connection
        .read
        .recv()
        .await
        .wrap_err("receiving Handshake")?
        .ok_or_else(|| eyre!("connection closed before receiving any messages"))?;

    msg!(match envelope {
        msg @ internode::Handshake => Ok((msg.node_no, msg.launch_id)),
        envelope => Err(unexpected_message_error(envelope)),
    })
}

fn make_regular(msg: impl Message) -> Envelope {
    Envelope::with_trace_id(
        msg,
        MessageKind::Regular { sender: Addr::NULL },
        scope::trace_id(),
    )
    .upcast()
}

fn unexpected_message_error(envelope: Envelope) -> eyre::Report {
    let message = envelope.message();
    eyre!(
        "unexpected message: {}::{}",
        message.protocol(),
        message.name()
    )
}
