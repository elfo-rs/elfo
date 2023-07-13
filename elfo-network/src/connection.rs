use std::{sync::Arc, time::Duration};

use eyre::Result;
use metrics::{decrement_gauge, increment_gauge};
use parking_lot::Mutex;
use quanta::Instant;
use tracing::{debug, error, trace, warn};

use elfo_core::{
    message, Local, Message,
    _priv::{EnvelopeOwned, GroupVisitor, MessageKind, Object, ObjectArc},
    errors::{RequestError, SendError, TrySendError},
    messages::{ConfigUpdated, Impossible},
    msg,
    node::NodeNo,
    remote, scope,
    stream::Stream,
    time::Interval,
    Addr, Context, Envelope, GroupNo, ResponseToken, Topology,
};
use elfo_utils::{likely, unlikely};

use self::{
    flows_rx::RxFlows,
    flows_tx::{Acquire, TryAcquire, TxFlows},
    requests::OutgoingRequests,
};
use crate::{
    codec::format::{NetworkEnvelope, NetworkEnvelopePayload},
    protocol::{internode, HandleConnection},
    rtt::Rtt,
    socket::{ReadHalf, WriteHalf},
    NetworkContext,
};

mod flow_control;
mod flows_rx;
mod flows_tx;
mod requests;

// TODO: send `CloseFlow` once an actor is closed, not only on incoming message.

#[message]
struct StartPusher(Local<Addr>);

#[message]
struct PusherStopped;

#[message]
struct PingTick;

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

        let time_origin = Instant::now();
        let tx_flows = Arc::new(TxFlows::new(first_message.initial_window));
        let rx_flows = Arc::new(Mutex::new(RxFlows::new(first_message.initial_window)));
        let requests = Arc::new(Mutex::new(OutgoingRequests::default()));
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
        let sw = SocketWriter {
            rx: local_rx,
            tx: socket.write,
            requests: requests.clone(),
        };
        self.ctx.attach(Stream::once(sw.exec()));

        // Start handling network incoming messages.
        let sr = SocketReader {
            ctx: self.ctx.pruned(),
            group_addr: self
                .topology
                .locals()
                .map(|g| g.addr)
                .find(|a| a.group_no() == self.local.0)
                .expect("invalid local group"),
            time_origin,
            // TODO: the number of samples should be calculated based on telemetry scrape
            //       interval, but it's not povideded for now by the elfo core.
            rtt: Rtt::new(5),
            rx: socket.read,
            tx: local_tx.clone(),
            tx_flows: tx_flows.clone(),
            rx_flows: rx_flows.clone(),
            requests,
        };
        self.ctx.attach(Stream::once(sr.exec()));

        // Start ping ticks.
        let ping_interval = self.ctx.attach(Interval::new(PingTick));
        ping_interval.start_after(Duration::ZERO, self.ctx.config().ping_interval);

        while let Some(envelope) = self.ctx.recv().await {
            // TODO: graceful termination
            // TODO: handle another `HandleConnection`

            msg!(match envelope {
                ConfigUpdated => {
                    ping_interval.set_period(self.ctx.config().ping_interval);
                }
                PingTick => {
                    let envelope = make_system_envelope(internode::Ping {
                        payload: time_origin.elapsed().as_nanos() as u64,
                    });
                    let _ = local_tx.try_send(KanalItem::simple(Addr::NULL, envelope));

                    // TODO: perform health check
                }
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

// === SocketWriter ===

/// A subtask that handles incoming messages from local actors and writes them
/// to the socket.
struct SocketWriter {
    rx: kanal::AsyncReceiver<KanalItem>,
    tx: WriteHalf,
    requests: Arc<Mutex<OutgoingRequests>>,
}

impl SocketWriter {
    async fn exec(mut self) -> Impossible {
        // We should write messages as many as possible at once to have better
        // compression rate and reduce the number of system calls.
        // On the other hand, we should minimize the time which every message is unsent.
        // Thus, we should find a balance between these two factors, some trade-off.
        // The current strategy is to send all available messages and then forcibly
        // flush intermediate buffers to the socket. So, frames besides the last
        // one (before the channel is empty) are complete.
        //
        // TODO: tokio implements budget on sockets, so this subtask sometimes returns
        // the execution back to the runtime even in case of a full incoming queue.
        // We should use `tokio::task::unconstrained()` here and preempt the (sub)task
        // after sending each batch of messages.
        loop {
            // TODO: error handling, metrics.
            let mut item = self.rx.recv().await.unwrap();
            loop {
                let (network_envelope, response_token) = make_network_envelope(item);
                scope::set_trace_id(network_envelope.trace_id);

                // This call can actually write to the socket if the buffer is full.
                let feed_future = self.tx.feed(&network_envelope);
                if feed_future.await.unwrap() {
                    // Store the response token only if the message is added.
                    // Otherwise, it will be dropped with the `Failed` reason.
                    if let Some(token) = response_token {
                        self.requests.lock().add_token(token);
                    }
                }

                item = ward!(self.rx.try_recv().unwrap(), break);
            }

            // Forcibly write to the socket remaining data in the buffer, because
            // we don't know how long we'll wait for the next message.
            self.tx.flush().await.unwrap();
        }
    }
}

fn make_network_envelope(item: KanalItem) -> (NetworkEnvelope, Option<ResponseToken>) {
    let (sender, trace_id, payload, token) = match (item.envelope, item.token) {
        // Regular, RequestAny, RequestAll
        (Ok(envelope), None) => {
            let sender = envelope.sender();
            let trace_id = envelope.trace_id();

            let (payload, token) = match envelope.message_kind() {
                MessageKind::Regular { .. } => (
                    NetworkEnvelopePayload::Regular {
                        message: envelope.unpack_regular(),
                    },
                    None,
                ),
                MessageKind::RequestAny(_) => {
                    let (message, token) = envelope.unpack_request();
                    (
                        NetworkEnvelopePayload::RequestAny {
                            request_id: token.request_id(),
                            message,
                        },
                        Some(token),
                    )
                }
                MessageKind::RequestAll(_) => {
                    let (message, token) = envelope.unpack_request();
                    (
                        NetworkEnvelopePayload::RequestAll {
                            request_id: token.request_id(),
                            message,
                        },
                        Some(token),
                    )
                }
                MessageKind::Response { .. } => unreachable!(),
            };

            (sender, trace_id, payload, token)
        }
        // Response
        (Ok(envelope), Some(token)) => {
            let sender = envelope.sender();
            let trace_id = envelope.trace_id();

            let payload = match envelope.message_kind() {
                MessageKind::Response { request_id, .. } => {
                    debug_assert_eq!(*request_id, token.request_id());
                    NetworkEnvelopePayload::Response {
                        request_id: *request_id,
                        message: Ok(envelope.unpack_regular()),
                        is_last: token.is_last(),
                    }
                }
                _ => unreachable!(),
            };

            // The token is semantically moved to another node.
            token.forget();

            (sender, trace_id, payload, None)
        }
        // Failed/Ignored Response
        (Err(err), Some(token)) => {
            let sender = Addr::NULL;
            let trace_id = token.trace_id();

            let payload = NetworkEnvelopePayload::Response {
                request_id: token.request_id(),
                message: Err(err),
                is_last: token.is_last(),
            };

            // The token is semantically moved to another node.
            token.forget();

            (sender, trace_id, payload, None)
        }
        (Err(_), None) => unreachable!(),
    };

    let envelope = NetworkEnvelope {
        sender,
        recipient: item.recipient,
        trace_id,
        payload,
    };

    (envelope, token)
}

// === SocketReader ===

/// A subtask that reads messages from the socket and routes them to local
/// groups. If some messages cannot be sent right now, the pusher is spawned.
struct SocketReader {
    ctx: Context,
    group_addr: Addr,
    time_origin: Instant,
    rtt: Rtt,
    rx: ReadHalf,
    tx: kanal::AsyncSender<KanalItem>,
    tx_flows: Arc<TxFlows>,
    rx_flows: Arc<Mutex<RxFlows>>,
    requests: Arc<Mutex<OutgoingRequests>>,
}

impl SocketReader {
    async fn exec(mut self) -> Impossible {
        // TODO: error handling.
        while let Some(network_envelope) = self.rx.recv().await.unwrap() {
            scope::set_trace_id(network_envelope.trace_id);

            let (sender, recipient) = (network_envelope.sender, network_envelope.recipient);
            let envelope = ward!(self.make_envelope(network_envelope), continue);

            // System messages have a special handling.
            if unlikely(self.handle_system_message(&envelope)) {
                continue;
            }

            // Recipients can respond to the sender, so we should add a flow.
            self.tx_flows.add_flow_if_needed(sender);

            // `NULL` means we should route to the group.
            if recipient == Addr::NULL {
                self.handle_routed_message(envelope);
            } else {
                self.handle_direct_message(recipient, envelope);
            }
        }

        todo!()
    }

    fn make_envelope(&self, network_envelope: NetworkEnvelope) -> Option<Envelope> {
        let (message, message_kind) = match network_envelope.payload {
            NetworkEnvelopePayload::Regular { message } => (
                message,
                MessageKind::Regular {
                    sender: network_envelope.sender,
                },
            ),
            NetworkEnvelopePayload::RequestAny {
                request_id,
                message,
            } => {
                let token = ResponseToken::new(
                    network_envelope.sender,
                    request_id,
                    network_envelope.trace_id,
                    self.ctx.book().clone(),
                );
                (message, MessageKind::RequestAny(token))
            }
            NetworkEnvelopePayload::RequestAll {
                request_id,
                message,
            } => {
                let token = ResponseToken::new(
                    network_envelope.sender,
                    request_id,
                    network_envelope.trace_id,
                    self.ctx.book().clone(),
                );
                (message, MessageKind::RequestAll(token))
            }
            NetworkEnvelopePayload::Response {
                request_id,
                message,
                is_last,
            } => {
                let sender = network_envelope.sender;
                let trace_id = network_envelope.trace_id;
                let recipient = network_envelope.recipient;

                let Some(token) = self
                    .requests
                    .lock()
                    .get_token(recipient, request_id, is_last)
                else {
                    warn!(
                        message = "received response to unknown request",
                        recipient = %recipient,
                        request_id = ?request_id,
                        is_last = is_last,
                    );
                    return None;
                };

                let Some(object) = self.ctx.book().get(recipient) else {
                    debug!(
                        message = "received response, but requester has gone",
                        recipient = %recipient,
                        request_id = ?request_id,
                        is_last = is_last,
                    );
                    return None;
                };

                let envelope = message.map(|message| {
                    Envelope::with_trace_id(
                        message,
                        MessageKind::Response { sender, request_id },
                        trace_id,
                    )
                });

                object.respond(token, envelope);
                return None;
            }
        };

        Some(Envelope::with_trace_id(
            message,
            message_kind,
            network_envelope.trace_id,
        ))
    }

    fn handle_system_message(&mut self, envelope: &Envelope) -> bool {
        msg!(match envelope {
            msg @ internode::UpdateFlow => {
                self.tx_flows.update_flow(msg);
            }
            msg @ internode::CloseFlow => {
                self.tx_flows.close_flow(msg);
            }
            msg @ internode::Ping => {
                let envelope = make_system_envelope(internode::Pong {
                    payload: msg.payload,
                });
                let _ = self.tx.try_send(KanalItem::simple(Addr::NULL, envelope));
            }
            msg @ internode::Pong => {
                let time_ns = self.time_origin.elapsed().as_nanos() as u64 - msg.payload;
                self.rtt.push(Duration::from_nanos(time_ns));
            }
            _ => return false,
        });

        true
    }

    fn handle_direct_message(&self, recipient: Addr, envelope: Envelope) {
        let book = self.ctx.book();
        let mut flows = self.rx_flows.lock();

        let Some(object) = book.get(recipient) else {
            if let Some(envelope) = flows.close(recipient).map(make_system_envelope) {
                self.tx
                    .try_send(KanalItem::simple(Addr::NULL, envelope))
                    .unwrap();
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
                let envelope = envelope.duplicate();
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
            self.tx
                .try_send(KanalItem::simple(Addr::NULL, envelope))
                .unwrap();
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
        match object.try_send(Addr::NULL, envelope) {
            Ok(()) => {
                if let Some(envelope) = flow.release_direct().map(make_system_envelope) {
                    let item = KanalItem::simple(Addr::NULL, envelope);
                    self.tx.try_send(item).unwrap();
                }
                if routed {
                    if let Some(envelope) = flows.release_routed().map(make_system_envelope) {
                        let item = KanalItem::simple(Addr::NULL, envelope);
                        self.tx.try_send(item).unwrap();
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
                    let item = KanalItem::simple(Addr::NULL, envelope);
                    self.tx.try_send(item).unwrap();
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

/// A subtask that pushes pending messages to an unstable actor.
struct Pusher {
    ctx: Context,
    actor_addr: Addr,
    tx: kanal::AsyncSender<KanalItem>,
    rx_flows: Arc<Mutex<RxFlows>>,
}

impl Pusher {
    async fn exec(self) -> PusherStopped {
        debug!(actor_addr = %self.actor_addr, "pusher started");
        increment_gauge!("elfo_network_pushers", 1.);

        loop {
            let Some((envelope, routed)) = self.rx_flows.lock().dequeue(self.actor_addr) else {
                break;
            };

            if !self.push(envelope, routed).await {
                let mut flows = self.rx_flows.lock();
                if let Some(envelope) = flows.close(self.actor_addr).map(make_system_envelope) {
                    self.tx
                        .try_send(KanalItem::simple(Addr::NULL, envelope))
                        .unwrap();
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
                self.tx
                    .try_send(KanalItem::simple(Addr::NULL, envelope))
                    .unwrap();
            }
            if routed {
                if let Some(envelope) = flows.release_routed().map(make_system_envelope) {
                    self.tx
                        .try_send(KanalItem::simple(Addr::NULL, envelope))
                        .unwrap();
                }
            }
            true
        } else {
            false
        }
    }
}

impl Drop for Pusher {
    fn drop(&mut self) {
        decrement_gauge!("elfo_network_pushers", 1.);
    }
}

// === RemoteHandle ===

struct KanalItem {
    recipient: Addr,
    envelope: Result<Envelope, RequestError>,
    token: Option<ResponseToken>,
}

impl KanalItem {
    fn simple(recipient: Addr, envelope: Envelope) -> Self {
        Self {
            recipient,
            envelope: Ok(envelope),
            token: None,
        }
    }
}

struct RemoteHandle {
    tx: kanal::AsyncSender<KanalItem>,
    tx_flows: Arc<TxFlows>,
}

impl remote::RemoteHandle for RemoteHandle {
    fn send(&self, recipient: Addr, envelope: Envelope) -> remote::SendResult {
        debug_assert!(!recipient.is_local());

        match self.tx_flows.acquire(recipient) {
            Acquire::Done => {
                let mut item = Some(KanalItem::simple(recipient, envelope));
                match self.tx.try_send_option(&mut item) {
                    Ok(true) => remote::SendResult::Ok,
                    Ok(false) => unreachable!(),
                    Err(_) => {
                        remote::SendResult::Err(SendError(item.take().unwrap().envelope.unwrap()))
                    }
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
                let mut item = Some(KanalItem::simple(recipient, envelope));
                match self.tx.try_send_option(&mut item) {
                    Ok(true) => Ok(()),
                    Ok(false) => unreachable!(),
                    Err(_) => Err(TrySendError::Closed(item.take().unwrap().envelope.unwrap())),
                }
            }
            TryAcquire::Full => Err(TrySendError::Full(envelope)),
            TryAcquire::Closed => Err(TrySendError::Closed(envelope)),
        }
    }

    fn respond(&self, token: ResponseToken, envelope: Result<Envelope, RequestError>) {
        debug_assert!(!token.is_forgotten());
        let recipient = token.sender();
        debug_assert!(recipient.is_remote());

        if likely(self.tx_flows.do_acquire(recipient)) {
            let item = KanalItem {
                recipient,
                envelope,
                token: Some(token),
            };
            match self.tx.try_send(item) {
                Ok(true) => return,
                Ok(false) => unreachable!(),
                Err(_) => {}
            }
        }

        trace!(addr = %recipient, "flow is closed, response is lost");
    }
}
