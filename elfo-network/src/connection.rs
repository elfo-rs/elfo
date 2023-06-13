use std::sync::Arc;

use eyre::Result;

use elfo_core::{
    _priv::{AnyMessage, MessageKind},
    errors::{SendError, TrySendError},
    messages::Impossible,
    msg,
    node::NodeNo,
    remote, scope,
    stream::Stream,
    Addr, Context, Envelope, GroupNo, Topology, UnattachedSource,
};
use elfo_utils::unlikely;

use self::flows_tx::{Acquire, TryAcquire, TxFlows};
use crate::{
    codec::{NetworkEnvelope, NetworkMessageKind},
    protocol::{internode, HandleConnection},
    socket::{ReadHalf, WriteHalf},
    NetworkContext,
};

mod flow_control;
mod flows_tx;

pub(crate) struct Connection {
    ctx: NetworkContext,
    topology: Topology,
    local: (GroupNo, String),
    remote: (NodeNo, GroupNo, String),
    flows: Arc<TxFlows>,
}

impl Connection {
    pub(super) fn new(
        ctx: NetworkContext,
        local: (GroupNo, String),
        remote: (NodeNo, GroupNo, String),
        topology: Topology,
    ) -> Self {
        Self {
            ctx,
            topology,
            local,
            remote,
            flows: Default::default(),
        }
    }

    pub(super) async fn main(mut self) -> Result<()> {
        // Receive the socket. It must always be a first message.
        let first_message = msg!(match self.ctx.try_recv().await? {
            msg @ HandleConnection => msg,
            _ => unreachable!("unexpected initial message"),
        });

        self.flows.configure(first_message.initial_window);
        let socket = first_message.socket.take().unwrap();

        // Register `RemoteHandle`. Now we can receive messages from local groups.
        let (local_tx, local_rx) = kanal::unbounded_async();
        let remote_handle = RemoteHandle {
            tx: local_tx,
            flows: self.flows.clone(),
        };
        let _guard = self.topology.register_remote(
            self.local.0,
            (self.remote.0, self.remote.1),
            &self.remote.2,
            remote_handle,
        );

        // Start handling local incoming messages.
        self.ctx
            .attach(make_local_rx_handler(local_rx, socket.write));

        // Start handling network incoming messages.
        let sr = SocketReader {
            ctx: self.ctx.pruned(),
            rx: socket.read,
            group_addr: self
                .topology
                .locals()
                .map(|g| g.addr)
                .find(|a| a.group_no() == self.local.0)
                .expect("invalid local group"),
            tx_flows: self.flows.clone(),
        };
        self.ctx.attach(Stream::once(sr.exec()));

        while let Some(_envelope) = self.ctx.recv().await {
            // TODO: graceful termination
            // TODO: handle another `HandleConnection`
        }

        Ok(())
    }
}

fn make_local_rx_handler(
    rx: kanal::AsyncReceiver<(Addr, Envelope)>,
    mut tx: WriteHalf,
) -> UnattachedSource<Stream<Impossible>> {
    // We should write messages as many as possible at once to have better
    // compression rate and reduce the number of system calls.
    // On the other hand, we should minimize the time which every message is unsent.
    // Thus, we should find a balance between these two factors, some trade-off.
    // The current strategy is to send all available messages and then forcibly
    // flush intermediate buffers to the socket.
    //
    // Also, tokio implements budget on sockets, so this subtask sometimes returns
    // the execution back to the runtime even in case of a full incoming queue.
    Stream::once(async move {
        loop {
            // TODO: trace_id (?), error handling, metrics.
            let (mut recipient, mut envelope) = rx.recv().await.unwrap();
            loop {
                let network_envelope = NetworkEnvelope {
                    sender: envelope.sender(),
                    recipient,
                    trace_id: envelope.trace_id(),
                    kind: match envelope.message_kind() {
                        MessageKind::Regular { .. } => NetworkMessageKind::Regular,
                        _ => todo!(),
                    },
                    message: envelope.into_message(),
                };

                // This call can actually write to the socket if the buffer is full.
                tx.send(network_envelope).await.unwrap();
                (recipient, envelope) = ward!(rx.try_recv().unwrap(), break);
            }

            // Forcibly write to the socket remaining data in the buffer, because
            // we don't know how long we'll wait for the next message.
            tx.flush().await.unwrap();
        }
    })
}

// === SocketReader ===

/// Subtask that reads messages from the socket and routes them to local groups.
struct SocketReader {
    ctx: Context,
    rx: ReadHalf,
    group_addr: Addr,
    tx_flows: Arc<TxFlows>,
}

impl SocketReader {
    async fn exec(mut self) -> Impossible {
        // TODO: error handling.
        while let Some(mut network_envelope) = self.rx.recv().await.unwrap() {
            scope::set_trace_id(network_envelope.trace_id);

            // System messages have a special handling.
            if unlikely(self.handle_system_message(&network_envelope.message)) {
                continue;
            }

            // Recipients can respond to the sender, so we should add a flow.
            self.tx_flows.add_flow_if_needed(network_envelope.sender);

            // `NULL` means we should route to the group.
            if network_envelope.recipient == Addr::NULL {
                network_envelope.recipient = self.group_addr;
            }
            assert!(
                matches!(network_envelope.kind, NetworkMessageKind::Regular),
                "only regular messages are supported for now"
            );

            // TODO: backpressure + spawn as another source.
            self.ctx
                .do_send_to(
                    network_envelope.recipient,
                    network_envelope.message,
                    MessageKind::Regular {
                        sender: network_envelope.sender,
                    },
                )
                .await // TODO
                .unwrap();
        }

        todo!()
    }

    fn handle_system_message(&self, message: &AnyMessage) -> bool {
        if let Some(msg) = message.downcast_ref::<internode::UpdateFlow>() {
            self.tx_flows.update_flow(msg);
            true
        } else if let Some(msg) = message.downcast_ref::<internode::CloseFlow>() {
            self.tx_flows.close_flow(msg);
            true
        } else {
            // TODO: ping/pong
            false
        }
    }
}

// === RemoteHandle ===

struct RemoteHandle {
    tx: kanal::AsyncSender<(Addr, Envelope)>,
    flows: Arc<TxFlows>,
}

impl remote::RemoteHandle for RemoteHandle {
    fn send(&self, recipient: Addr, envelope: Envelope) -> remote::SendResult {
        match self.flows.acquire(recipient) {
            Acquire::Done => {
                let mut item = Some((recipient, envelope));
                match self.tx.try_send_option(&mut item) {
                    Ok(true) => remote::SendResult::Ok,
                    Ok(false) => unreachable!(),
                    Err(_) => remote::SendResult::Err(SendError(item.take().unwrap().1)),
                }
            }
            Acquire::Full(notified) => remote::SendResult::Wait(notified, envelope),
            Acquire::Closed => remote::SendResult::Err(SendError(envelope)),
        }
    }

    fn try_send(&self, recipient: Addr, envelope: Envelope) -> Result<(), TrySendError<Envelope>> {
        match self.flows.try_acquire(recipient) {
            TryAcquire::Done => {
                let mut item = Some((recipient, envelope));
                match self.tx.try_send_option(&mut item) {
                    Ok(true) => Ok(()),
                    Ok(false) => unreachable!(),
                    Err(_) => Err(TrySendError::Closed(item.take().unwrap().1)),
                }
            }
            TryAcquire::Full => Err(TrySendError::Full(envelope)),
            TryAcquire::Closed => Err(TrySendError::Closed(envelope)),
        }
    }

    fn unbounded_send(
        &self,
        recipient: Addr,
        envelope: Envelope,
    ) -> Result<(), SendError<Envelope>> {
        if !self.flows.do_acquire(recipient) {
            return Err(SendError(envelope));
        }

        let mut item = Some((recipient, envelope));
        match self.tx.try_send_option(&mut item) {
            Ok(true) => Ok(()),
            Ok(false) => unreachable!(),
            Err(_) => Err(SendError(item.take().unwrap().1)),
        }
    }
}
