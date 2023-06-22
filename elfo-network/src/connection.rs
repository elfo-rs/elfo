use std::sync::Arc;

use eyre::Result;
use parking_lot::Mutex;
use tracing::{debug, error};

use elfo_core::{
    message, Local, Message,
    _priv::{GroupVisitor, MessageKind, Object, ObjectArc},
    errors::{SendError, TrySendError},
    messages::Impossible,
    msg,
    node::NodeNo,
    remote, scope,
    stream::Stream,
    Addr, Context, Envelope, GroupNo, Topology, UnattachedSource,
};
use elfo_utils::unlikely;

use self::{
    flows_rx::RxFlows,
    flows_tx::{Acquire, TryAcquire, TxFlows},
};
use crate::{
    codec::{NetworkEnvelope, NetworkMessageKind},
    protocol::{internode, HandleConnection},
    socket::{ReadHalf, WriteHalf},
    NetworkContext,
};

mod flow_control;
mod flows_rx;
mod flows_tx;

// TODO: send `CloseFlow` once an actor is closed, not only on incoming message.

#[message]
struct StartPusher(Local<Addr>);

#[message]
struct PusherStopped;

pub(crate) struct Connection {
    ctx: NetworkContext,
    topology: Topology,
    local: (GroupNo, String),
    remote: (NodeNo, GroupNo, String),
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
        }
    }

    pub(super) async fn main(mut self) -> Result<()> {
        // Receive the socket. It must always be a first message.
        let first_message = msg!(match self.ctx.try_recv().await? {
            msg @ HandleConnection => msg,
            _ => unreachable!("unexpected initial message"),
        });

        let tx_flows = Arc::new(TxFlows::new(first_message.initial_window));
        let rx_flows = Arc::new(Mutex::new(RxFlows::new(first_message.initial_window)));
        let socket = first_message.socket.take().unwrap();

        // Register `RemoteHandle`. Now we can receive messages from local groups.
        let (local_tx, local_rx) = kanal::unbounded_async();
        let remote_handle = RemoteHandle {
            tx: local_tx.clone(),
            tx_flows: tx_flows.clone(),
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
            group_addr: self
                .topology
                .locals()
                .map(|g| g.addr)
                .find(|a| a.group_no() == self.local.0)
                .expect("invalid local group"),
            rx: socket.read,
            tx: local_tx.clone(),
            tx_flows: tx_flows.clone(),
            rx_flows: rx_flows.clone(),
        };
        self.ctx.attach(Stream::once(sr.exec()));

        while let Some(envelope) = self.ctx.recv().await {
            // TODO: graceful termination
            // TODO: handle another `HandleConnection`

            msg!(match envelope {
                StartPusher(addr) => {
                    let pusher = Pusher {
                        ctx: self.ctx.pruned(),
                        actor_addr: *addr,
                        tx: local_tx.clone(),
                        rx_flows: rx_flows.clone(),
                    };

                    self.ctx.attach(Stream::once(pusher.exec()));
                }
            });
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

/// A subtask that reads messages from the socket and routes them to local
/// groups.
struct SocketReader {
    ctx: Context,
    group_addr: Addr,
    rx: ReadHalf,
    tx: kanal::AsyncSender<(Addr, Envelope)>,
    tx_flows: Arc<TxFlows>,
    rx_flows: Arc<Mutex<RxFlows>>,
}

impl SocketReader {
    async fn exec(mut self) -> Impossible {
        // TODO: error handling.
        while let Some(network_envelope) = self.rx.recv().await.unwrap() {
            scope::set_trace_id(network_envelope.trace_id);

            assert!(
                matches!(network_envelope.kind, NetworkMessageKind::Regular),
                "only regular messages are supported for now"
            );

            let envelope = Envelope::with_trace_id(
                network_envelope.message,
                MessageKind::Regular {
                    sender: network_envelope.sender,
                },
                network_envelope.trace_id,
            );

            // System messages have a special handling.
            if unlikely(self.handle_system_message(&envelope)) {
                continue;
            }

            // Recipients can respond to the sender, so we should add a flow.
            self.tx_flows.add_flow_if_needed(network_envelope.sender);

            // `NULL` means we should route to the group.
            if network_envelope.recipient == Addr::NULL {
                self.handle_routed_message(envelope);
            } else {
                self.handle_direct_message(network_envelope.recipient, envelope);
            }
        }

        todo!()
    }

    fn handle_system_message(&self, envelope: &Envelope) -> bool {
        msg!(match envelope {
            msg @ internode::UpdateFlow => {
                self.tx_flows.update_flow(msg);
                true
            }
            msg @ internode::CloseFlow => {
                self.tx_flows.close_flow(msg);
                true
            }
            // TODO: Ping/Pong
            _ => {
                false
            }
        })
    }

    fn handle_direct_message(&self, recipient: Addr, envelope: Envelope) {
        let book = self.ctx.book();
        let mut flows = self.rx_flows.lock();

        let Some(object) = book.get(recipient) else {
            if let Some(envelope) = flows.close(recipient).map(make_system_envelope) {
                self.tx.try_send((Addr::NULL, envelope)).unwrap();
            }
            return;
        };

        self.do_handle_message(&mut flows, &object, envelope, false)
    }

    fn handle_routed_message(&self, envelope: Envelope) {
        struct TrySendGroupVisitor<'a> {
            this: &'a SocketReader,
            flows: &'a mut RxFlows,
        }

        impl GroupVisitor for TrySendGroupVisitor<'_> {
            fn done(&mut self) {}

            fn empty(&mut self, _envelope: Envelope) {
                // TODO: maybe emit some metric?
            }

            fn visit(&mut self, object: &ObjectArc, envelope: &Envelope) {
                let envelope = envelope.duplicate(self.this.ctx.book()).unwrap();
                self.this
                    .do_handle_message(self.flows, object, envelope, true);
            }

            fn visit_last(&mut self, object: &ObjectArc, envelope: Envelope) {
                self.this
                    .do_handle_message(self.flows, object, envelope, true);
            }
        }

        let mut flows = self.rx_flows.lock();
        flows.acquire_routed(true);

        let mut visitor = TrySendGroupVisitor {
            this: self,
            flows: &mut flows,
        };

        let group = self
            .ctx
            .book()
            .get(self.group_addr)
            .expect("invalid local group addr");
        group.visit_group(envelope, &mut visitor);

        if let Some(envelope) = flows.release_routed().map(make_system_envelope) {
            self.tx.try_send((Addr::NULL, envelope)).unwrap();
        }
    }

    fn do_handle_message(
        &self,
        flows: &mut RxFlows,
        object: &Object,
        envelope: Envelope,
        routed: bool,
    ) {
        if routed {
            flows.acquire_routed(false);
        }

        let mut flow = flows.get_or_create_flow(object.addr());
        flow.acquire_direct(!routed);

        // If the recipient already has pending messages, enqueue and return.
        if !flow.is_stable() {
            flow.enqueue(envelope, false);
            return;
        }

        // Otherwise, try to send the message right now.
        match object.try_send(&self.ctx, Addr::NULL, envelope) {
            Ok(()) => {
                if let Some(envelope) = flow.release_direct().map(make_system_envelope) {
                    self.tx.try_send((Addr::NULL, envelope)).unwrap();
                }
                if routed {
                    if let Some(envelope) = flows.release_routed().map(make_system_envelope) {
                        self.tx.try_send((Addr::NULL, envelope)).unwrap();
                    }
                }
            }
            Err(TrySendError::Full(envelope)) => {
                flow.enqueue(envelope, false);

                // Start a pusher for this actor.
                let msg = StartPusher(object.addr().into());
                if let Err(err) = self.ctx.try_send_to(self.ctx.addr(), msg) {
                    error!(error = %err, "failed to start a pusher");
                }
            }
            Err(TrySendError::Closed(_)) => {
                if let Some(envelope) = flows.close(object.addr()).map(make_system_envelope) {
                    self.tx.try_send((Addr::NULL, envelope)).unwrap();
                }
            }
        }
    }
}

fn make_system_envelope(message: impl Message) -> Envelope {
    Envelope::new(
        message.upcast(),
        MessageKind::Regular { sender: Addr::NULL },
    )
}

// === Pusher ===

/// A subtast that pushes pending messages to an unstable actor.
struct Pusher {
    ctx: Context,
    actor_addr: Addr,
    tx: kanal::AsyncSender<(Addr, Envelope)>,
    rx_flows: Arc<Mutex<RxFlows>>,
}

impl Pusher {
    async fn exec(self) -> PusherStopped {
        debug!(actor_addr = %self.actor_addr, "pusher started");

        loop {
            let Some((envelope, routed)) = self.rx_flows.lock().dequeue(self.actor_addr) else {
                break;
            };

            if !self.push(envelope, routed).await {
                let mut flows = self.rx_flows.lock();
                if let Some(envelope) = flows.close(self.actor_addr).map(make_system_envelope) {
                    self.tx.try_send((Addr::NULL, envelope)).unwrap();
                }
                break;
            }
        }

        debug!(actor_addr = %self.actor_addr, "pusher stopped");
        PusherStopped
    }

    async fn push(&self, envelope: Envelope, routed: bool) -> bool {
        let Some(object) = self.ctx.book().get_owned(self.actor_addr) else {
            return false;
        };

        if object.send(&self.ctx, Addr::NULL, envelope).await.is_ok() {
            let mut flows = self.rx_flows.lock();

            let Some(mut flow) = flows.get_flow(self.actor_addr) else {
                return false;
            };

            if let Some(envelope) = flow.release_direct().map(make_system_envelope) {
                self.tx.try_send((Addr::NULL, envelope)).unwrap();
            }
            if routed {
                if let Some(envelope) = flows.release_routed().map(make_system_envelope) {
                    self.tx.try_send((Addr::NULL, envelope)).unwrap();
                }
            }
            true
        } else {
            false
        }
    }
}

// === RemoteHandle ===

struct RemoteHandle {
    tx: kanal::AsyncSender<(Addr, Envelope)>,
    tx_flows: Arc<TxFlows>,
}

impl remote::RemoteHandle for RemoteHandle {
    fn send(&self, recipient: Addr, envelope: Envelope) -> remote::SendResult {
        debug_assert!(!recipient.is_local());

        match self.tx_flows.acquire(recipient) {
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
        debug_assert!(!recipient.is_local());

        match self.tx_flows.try_acquire(recipient) {
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
        debug_assert!(!recipient.is_local());

        if !self.tx_flows.do_acquire(recipient) {
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
