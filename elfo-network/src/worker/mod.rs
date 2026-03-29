use std::{future::Future, sync::Arc, time::Duration};

use eyre::Result;
use flows_rx::RxFlowEvent;
use futures::future::{ready, Either};
use metrics::{decrement_gauge, histogram, increment_gauge};
use parking_lot::Mutex;
use tracing::{debug, error, info, trace, warn};

use elfo_core::{
    addr::{Addr, NodeNo},
    message, Local, Message,
    _priv::{AnyMessage, EbrGuard, GroupVisitor, MessageKind, Object, OwnedObject},
    errors::{RequestError, SendError, TrySendError},
    messages::{ConfigUpdated, Impossible},
    msg, remote, scope,
    stream::Stream,
    time::Interval,
    Context, Envelope, ResponseToken, Topology,
};
use elfo_utils::{likely, time::Instant, unlikely};

use self::{
    flows_rx::RxFlows,
    flows_tx::{Acquire, TryAcquire, TxFlows, TxFlowsAcquirer},
    requests::OutgoingRequests,
};

use crate::{
    codec::{
        decode::EnvelopeDetails,
        format::{
            NetworkAddr, NetworkEnvelope, NetworkEnvelopePayload, KIND_REQUEST_ALL,
            KIND_REQUEST_ANY, KIND_RESPONSE_FAILED, KIND_RESPONSE_IGNORED, KIND_RESPONSE_OK,
        },
    },
    frame::write::FrameState,
    protocol::{internode, AbortConnection, ConnId, ConnectionFailed, GroupMeta, HandleConnection},
    rtt::Rtt,
    socket::{ReadError, ReadHalf, WriteHalf},
    NetworkContext,
};

mod flow_control;
mod flows_rx;
mod flows_tx;
mod requests;

// TODO: send `CloseFlow` once an actor is closed, not only on incoming message.
// TODO: don't send control messages if the peer knows nothing about the flow.

#[message]
struct StartPusher(Local<Addr>);

#[message]
struct PusherStopped;

#[message]
struct PingTick;

#[message]
struct ConnectionClosed;

struct FailGuard {
    ctx: Context,
    id: ConnId,
}

impl Drop for FailGuard {
    fn drop(&mut self) {
        let Self { ctx, id } = self;
        _ = ctx.unbounded_send_to(ctx.group(), ConnectionFailed { id: *id });
    }
}

pub(crate) struct Worker {
    ctx: NetworkContext,
    topology: Topology,
    local: GroupMeta,
    remote: GroupMeta,
}

impl Worker {
    pub(super) fn new(
        ctx: NetworkContext,
        local: GroupMeta,
        remote: GroupMeta,
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
        let _fail_guard = FailGuard {
            ctx: self.ctx.pruned(),
            id: first_message.id,
        };

        let time_origin = Instant::now();
        let (tx_flows, tx_flows_acquirer) = TxFlows::new(first_message.initial_window);
        let rx_flows = Arc::new(Mutex::new(RxFlows::new(
            self.local.node_no,
            first_message.initial_window,
        )));
        let requests = Arc::new(Mutex::new(OutgoingRequests::default()));
        let socket = {
            let mut socket = first_message.socket.take().unwrap();
            socket.enable_transport_specific_metrics(self.ctx.config().transport_specific_metrics);
            socket
        };

        info!(
            message = "connection picked up",
            socket = %socket.info,
            peer = %socket.peer,
            capabilities = %socket.capabilities,
        );

        // Register `RemoteHandle`. Now we can receive messages from local groups.
        let (local_tx, local_rx) = kanal::unbounded_async();
        let remote_handle = RemoteHandle {
            tx: local_tx.clone(),
            tx_flows: tx_flows_acquirer,
        };
        let remote_group_guard = self.topology.register_remote(
            self.ctx.addr(),
            self.local.group_no,
            (self.remote.node_no, self.remote.group_no),
            &self.remote.group_name,
            remote_handle,
        );

        // Start handling local incoming messages.
        let sw = SocketWriter {
            node_no: self.local.node_no,
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
                .find(|a| a.group_no() == Some(self.local.group_no))
                .expect("invalid local group"),
            handle_addr: remote_group_guard.handle_addr(),
            time_origin,
            // TODO: the number of samples should be calculated based on telemetry scrape
            //       interval, but it's not povideded for now by the elfo core.
            rtt: Rtt::new(5),
            rx: socket.read,
            tx: local_tx.clone(),
            tx_flows,
            rx_flows: rx_flows.clone(),
            requests,
        };
        self.ctx.attach(Stream::once(sr.exec()));

        let mut idle = socket.idle;

        // Start ping ticks.
        let ping_interval = self.ctx.attach(Interval::new(PingTick));
        ping_interval.start_after(Duration::ZERO, self.ctx.config().ping_interval);

        while let Some(envelope) = self.ctx.recv().await {
            // TODO: graceful termination

            msg!(match envelope {
                ConfigUpdated => {
                    ping_interval.set_period(self.ctx.config().ping_interval);
                }
                PingTick => {
                    let idle_time = idle.check();

                    if idle_time >= self.ctx.config().idle_timeout {
                        error!(
                            message = "no data is received for a long time, closing",
                            idle_time = ?idle_time,
                            timeout = ?self.ctx.config().idle_timeout,
                        );
                        break;
                    }

                    let envelope = make_system_envelope(internode::Ping {
                        payload: time_origin.elapsed_nanos(),
                    });
                    let _ = local_tx.try_send(KanalItem::unbounded(NetworkAddr::NULL, envelope));
                }

                HandleConnection { socket, .. } => {
                    let socket = socket.take().unwrap();
                    info!(
                        message = "duplicate connection, skipping",
                        socket = %socket.info,
                        peer = %socket.peer,
                        capabilities = %socket.capabilities,
                    );
                }
                AbortConnection { id, .. } => {
                    if id == first_message.id {
                        info!("connection aborted due to internal request");
                        break;
                    }
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
                ConnectionClosed => {
                    info!("connection closed by peer");
                    break;
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
    node_no: NodeNo,
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
            // NOTE: We use `unwrap()` for results from all `self.tx` methods because these
            // errors are unrecoverable anyway, although it's not a good practice.
            let mut item = self.rx.recv().await.unwrap();
            loop {
                if let Ok(envelope) = &item.envelope {
                    // NOTE: Consider using `context/stats.rs` once handling time is also added.
                    histogram!(
                        "elfo_message_waiting_time_seconds",
                        envelope.created_time().elapsed_secs_f64(),
                    );
                }

                let (network_envelope, response_token) = make_network_envelope(item, self.node_no);
                scope::set_trace_id(network_envelope.trace_id);

                if let Some(frame_state) = self.tx.feed(&network_envelope).unwrap() {
                    // Envelope was encoded successfylly, so we can store the response token.
                    // Otherwise, it will be dropped with the `Failed` reason.
                    if let Some(token) = response_token {
                        self.requests.lock().add_token(token);
                    }

                    if frame_state == FrameState::FlushAdvised {
                        break;
                    }
                }

                item = ward!(self.rx.try_recv().unwrap(), break);
            }

            // We have either received a recommendation for a flush or there are no more
            // messages for the time being. Since we don't know how long we'll
            // wait for the next message, we flush in both cases.
            self.tx.flush().await.unwrap();
        }
    }
}

fn make_network_envelope(
    item: KanalItem,
    node_no: NodeNo,
) -> (NetworkEnvelope, Option<ResponseToken>) {
    let (sender, trace_id, payload, token) = match (item.envelope, item.token) {
        // Regular, RequestAny, RequestAll
        (Ok(envelope), None) => {
            let sender = envelope.sender();
            let trace_id = envelope.trace_id();
            let (message, kind) = envelope.unpack::<AnyMessage>().expect("impossible");

            let (payload, token) = match kind {
                MessageKind::Regular { .. } => (NetworkEnvelopePayload::Regular { message }, None),
                MessageKind::RequestAny(token) => (
                    NetworkEnvelopePayload::RequestAny {
                        request_id: token.request_id(),
                        message,
                    },
                    Some(token),
                ),
                MessageKind::RequestAll(token) => (
                    NetworkEnvelopePayload::RequestAll {
                        request_id: token.request_id(),
                        message,
                    },
                    Some(token),
                ),
                MessageKind::Response { .. } => unreachable!(),
            };

            (sender, trace_id, payload, token)
        }
        // Response
        (Ok(envelope), Some(token)) => {
            let sender = envelope.sender();
            let trace_id = envelope.trace_id();
            let (message, kind) = envelope.unpack::<AnyMessage>().expect("impossible");

            let payload = match kind {
                MessageKind::Response { request_id, .. } => {
                    debug_assert_eq!(request_id, token.request_id());
                    NetworkEnvelopePayload::Response {
                        request_id,
                        message: Ok(message),
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
        sender: NetworkAddr::from_local(sender, node_no),
        recipient: item.recipient,
        trace_id,
        payload,
        bounded: item.bounded,
    };

    (envelope, token)
}

// === SocketReader ===

/// A subtask that reads messages from the socket and routes them to local
/// groups. If some messages cannot be sent right now, the pusher is spawned.
struct SocketReader {
    ctx: Context,
    group_addr: Addr,
    handle_addr: Addr,
    time_origin: Instant,
    rtt: Rtt,
    rx: ReadHalf,
    tx: kanal::AsyncSender<KanalItem>,
    tx_flows: TxFlows,
    rx_flows: Arc<Mutex<RxFlows>>,
    requests: Arc<Mutex<OutgoingRequests>>,
}

impl SocketReader {
    async fn exec(mut self) -> ConnectionClosed {
        loop {
            let network_envelope = match self.rx.recv().await {
                Ok(Some(envelope)) => envelope,
                Ok(None) => break,
                Err(ReadError::EnvelopeSkipped(details)) => {
                    scope::set_trace_id(details.trace_id);
                    self.handle_skipped_message(details);
                    continue;
                }
                Err(ReadError::Fatal(err)) => {
                    // TODO: error handling.
                    panic!("fatal error while reading from socket: {err:#}");
                }
            };

            scope::set_trace_id(network_envelope.trace_id);

            let (sender, recipient) = (network_envelope.sender, network_envelope.recipient);
            let bounded = network_envelope.bounded;
            let envelope = ward!(self.make_envelope(network_envelope), continue);

            // System messages have a special handling.
            if unlikely(self.handle_system_message(&envelope)) {
                continue;
            }

            // Recipients can respond to the sender, so we should add a flow.
            self.tx_flows.add_flow_if_needed(sender);

            // `NULL` means we should route to the group.
            if recipient == NetworkAddr::NULL {
                self.handle_routed_message(envelope, bounded);
            } else {
                self.handle_direct_message(recipient.into_local(), envelope, bounded);
            }
        }

        ConnectionClosed
    }

    /// Ensures that messages that were skipped due to errors during decoding
    /// are properly accounted for in flow control. Also notifies the remote
    /// actor if the message was a request in order to avoid indefinite
    /// waiting from the remote actor's side.
    fn handle_skipped_message(&self, details: EnvelopeDetails) {
        let update = {
            let mut rx_flows = self.rx_flows.lock();
            if details.recipient == NetworkAddr::NULL {
                rx_flows.acquire_routed(true);
                rx_flows.release_routed()
            } else {
                // TODO: it's debatable that we should create a flow here.
                let mut rx_flow = rx_flows.get_or_create_flow(details.recipient.into_local());
                rx_flow.acquire_direct(true);
                rx_flow.release_direct()
            }
        };

        self.send_back(update);

        if details.kind == KIND_REQUEST_ALL || details.kind == KIND_REQUEST_ANY {
            let guard = EbrGuard::new();
            let sender = self
                .ctx
                .book()
                .get(self.handle_addr, &guard)
                .expect("bug: remote group is missing in the address book");

            let token = ResponseToken::new(
                details.sender.into_remote(),
                details.request_id.expect("bug: request_id is missing"),
                details.trace_id,
                self.ctx.book().clone(),
            );

            // This can be the first time we have received a message from this sender,
            // so we need to introduce the flow which will be used in `sender.respond()`
            // below.
            self.tx_flows.add_flow_if_needed(details.sender);
            sender.respond(token, Err(RequestError::Failed));
        } else if details.kind == KIND_RESPONSE_OK
            || details.kind == KIND_RESPONSE_FAILED
            || details.kind == KIND_RESPONSE_IGNORED
        {
            let Some(token) = self.requests.lock().get_token(
                details.recipient.into_remote(),
                details.request_id.expect("bug: request_id is missing"),
                true,
            ) else {
                warn!(
                    message = "received response to unknown request",
                    kind = %details.kind,
                    sender = %details.sender,
                    recipient = %details.recipient,
                    request_id = ?details.request_id,
                );
                return;
            };

            // Dropped token will notify the request sender that the request failed.
            drop(token);
        }
    }

    fn make_envelope(&self, network_envelope: NetworkEnvelope) -> Option<Envelope> {
        let sender = network_envelope.sender.into_remote();
        let recipient = network_envelope.recipient.into_local();
        let trace_id = network_envelope.trace_id;

        let (message, message_kind) = match network_envelope.payload {
            NetworkEnvelopePayload::Regular { message } => {
                (message, MessageKind::Regular { sender })
            }
            NetworkEnvelopePayload::RequestAny {
                request_id,
                message,
            } => {
                let token =
                    ResponseToken::new(sender, request_id, trace_id, self.ctx.book().clone());
                (message, MessageKind::RequestAny(token))
            }
            NetworkEnvelopePayload::RequestAll {
                request_id,
                message,
            } => {
                let token =
                    ResponseToken::new(sender, request_id, trace_id, self.ctx.book().clone());
                (message, MessageKind::RequestAll(token))
            }
            NetworkEnvelopePayload::Response {
                request_id,
                message,
                is_last,
            } => {
                // Adjust RX flow.
                {
                    let mut flows = self.rx_flows.lock();
                    if let Some(mut flow) = flows.get_flow(recipient) {
                        flow.acquire_direct(true);
                        // Responds are unbounded, so release immediately.
                        self.send_back(flow.release_direct());
                    }
                }

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

                let guard = EbrGuard::new();
                let Some(object) = self.ctx.book().get(recipient, &guard) else {
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

                let mut flows = self.rx_flows.lock();
                let Some(flow) = flows.get_flow(recipient) else {
                    // No flow -> no pusher -> realtime processing.
                    object.respond(token, envelope);
                    return None;
                };

                if let Err((token, envelope)) = flow.try_enqueue_response(token, envelope) {
                    // Since this is a response to a request which originated from this node,
                    // all the neccessary flows have been already added.
                    object.respond(token, envelope);
                }

                return None;
            }
        };

        Some(Envelope::with_trace_id(message, message_kind, trace_id))
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
                self.send_back(Some(internode::Pong {
                    payload: msg.payload,
                }));
            }
            msg @ internode::Pong => {
                let time_ns = self.time_origin.elapsed_nanos() - msg.payload;
                self.rtt.push(Duration::from_nanos(time_ns));
            }
            _ => return false,
        });

        true
    }

    fn handle_direct_message(&self, recipient: Addr, envelope: Envelope, bounded: bool) {
        let book = self.ctx.book();
        let mut flows = self.rx_flows.lock();

        let guard = EbrGuard::new();
        let Some(object) = book.get(recipient, &guard) else {
            let (close, update) = flows.close(recipient);
            self.send_back(close);
            self.send_back(update);
            return;
        };

        self.do_handle_message(&mut flows, &object, envelope, false, bounded)
    }

    fn handle_routed_message(&self, envelope: Envelope, bounded: bool) {
        struct TrySendGroupVisitor<'a> {
            this: &'a SocketReader,
            flows: &'a mut RxFlows,
            bounded: bool,
        }

        impl GroupVisitor for TrySendGroupVisitor<'_> {
            fn done(&mut self) {}

            fn empty(&mut self, _envelope: Envelope) {
                // TODO: maybe emit some metric?
            }

            fn visit(&mut self, object: &OwnedObject, envelope: &Envelope) {
                let envelope = envelope.duplicate();
                self.this
                    .do_handle_message(self.flows, object, envelope, true, self.bounded);
            }

            fn visit_last(&mut self, object: &OwnedObject, envelope: Envelope) {
                self.this
                    .do_handle_message(self.flows, object, envelope, true, self.bounded);
            }
        }

        let mut flows = self.rx_flows.lock();
        flows.acquire_routed(true);

        let mut visitor = TrySendGroupVisitor {
            this: self,
            flows: &mut flows,
            bounded,
        };

        let guard = EbrGuard::new();
        let group = self
            .ctx
            .book()
            .get(self.group_addr, &guard)
            .expect("invalid local group addr");
        group.visit_group(envelope, &mut visitor);

        self.send_back(flows.release_routed());
    }

    fn do_handle_message(
        &self,
        flows: &mut RxFlows,
        object: &Object,
        envelope: Envelope,
        routed: bool,
        bounded: bool,
    ) {
        if routed {
            flows.acquire_routed(false);
        }

        let flow = flows.get_flow(object.addr());
        let enqueue_result = match (flow, bounded) {
            (Some(flow), true) => flow.try_enqueue_send(envelope, routed),
            (Some(flow), false) => flow.try_enqueue_unbounded(envelope, routed),
            (None, _) => Err(envelope),
        };
        // The flow is stable - real-time processing.
        let Err(envelope) = enqueue_result else {
            return;
        };

        let result = if bounded {
            object.try_send(Addr::NULL, envelope)
        } else {
            // TODO: malicious client checks?
            object
                .unbounded_send(Addr::NULL, envelope)
                .map_err(|SendError(e)| TrySendError::Closed(e))
        };

        // If the recipient has gone, close the flow and return.
        if matches!(result, Err(TrySendError::Closed(_))) {
            let (close, update) = flows.close(object.addr());
            self.send_back(close);
            self.send_back(update);

            if routed {
                self.send_back(flows.release_routed());
            }
            return;
        }

        // The recipient is alive, so we should add a new flow if it doesn't exist yet.
        let mut flow = flows.get_or_create_flow(object.addr());
        flow.acquire_direct(!routed);

        match result {
            Ok(()) => {
                self.send_back(flow.release_direct());

                if routed {
                    self.send_back(flows.release_routed());
                }
            }
            Err(TrySendError::Full(envelope)) => {
                if flow.enqueue_send(envelope, routed) {
                    let msg = StartPusher(object.addr().into());
                    if let Err(e) = self.ctx.try_send_to(self.ctx.addr(), msg) {
                        error!(error = %e, "failed to start a pusher");
                    }
                }
            }
            Err(TrySendError::Closed(..)) => unreachable!(),
        }
    }

    fn send_back(&self, message: Option<impl Message>) {
        if let Some(envelope) = message.map(make_system_envelope) {
            self.tx
                .try_send(KanalItem::unbounded(NetworkAddr::NULL, envelope))
                .unwrap();
        }
    }
}

fn make_system_envelope(message: impl Message) -> Envelope {
    Envelope::new(message, MessageKind::regular(Addr::NULL))
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
            let Some((event, routed)) = self.rx_flows.lock().dequeue(self.actor_addr) else {
                break;
            };

            if !self.push(event, routed).await {
                let mut flows = self.rx_flows.lock();
                let (close, update) = flows.close(self.actor_addr);
                self.send_back(close);
                self.send_back(update);

                if routed {
                    self.send_back(flows.release_routed());
                }
                break;
            }
        }

        debug!(actor_addr = %self.actor_addr, "pusher stopped");
        PusherStopped
    }

    fn handle_send_outcome(&self, outcome: Result<(), SendError<Envelope>>, routed: bool) -> bool {
        if outcome.is_err() {
            return false;
        }

        let mut flows = self.rx_flows.lock();
        let Some(mut flow) = flows.get_flow(self.actor_addr) else {
            return false;
        };

        self.send_back(flow.release_direct());

        if routed {
            self.send_back(flows.release_routed());
        }

        true
    }

    // Sadly we live in rust.
    fn blocking_send(
        &self,
        envelope: Envelope,
    ) -> impl Future<Output = Result<(), SendError<Envelope>>> + Send + 'static {
        let guard = EbrGuard::new();
        let Some(object) = self.ctx.book().get(self.actor_addr, &guard) else {
            return Either::Left(ready(Ok(())));
        };

        let send = Object::send(object, Addr::NULL, envelope);
        Either::Right(send)
    }

    async fn push(&self, event: RxFlowEvent, routed: bool) -> bool {
        // Sadly we live in rust, `EbrGuard: !Send`, thus writing
        let outcome = match event {
            RxFlowEvent::Message {
                envelope,
                bounded: false,
            } => {
                let guard = EbrGuard::new();
                let object = ward!(self.ctx.book().get(self.actor_addr, &guard), return false);

                object.unbounded_send(Addr::NULL, envelope)
            }
            RxFlowEvent::Message {
                envelope,
                bounded: true,
            } => {
                self.blocking_send(envelope).await
                //                          ^^^^^^ This await
                //                          will fail to compile.
                // And there's no prettier and shorter way than just to copy-paste code for
                // guard.
            }
            RxFlowEvent::Response { token, response } => {
                let guard = EbrGuard::new();
                let object = ward!(self.ctx.book().get(self.actor_addr, &guard), return false);

                object.respond(token, response);

                Ok(())
            }
        };

        self.handle_send_outcome(outcome, routed)
    }

    fn send_back(&self, message: Option<impl Message>) {
        if let Some(envelope) = message.map(make_system_envelope) {
            self.tx
                .try_send(KanalItem::unbounded(NetworkAddr::NULL, envelope))
                .unwrap();
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
    recipient: NetworkAddr,
    envelope: Result<Envelope, RequestError>,
    bounded: bool,
    token: Option<ResponseToken>,
}

impl KanalItem {
    fn unbounded(recipient: NetworkAddr, envelope: Envelope) -> Self {
        Self {
            recipient,
            envelope: Ok(envelope),
            token: None,
            bounded: false,
        }
    }

    fn bounded(recipient: NetworkAddr, envelope: Envelope) -> Self {
        Self {
            recipient,
            envelope: Ok(envelope),
            token: None,
            bounded: true,
        }
    }
}

struct RemoteHandle {
    tx: kanal::AsyncSender<KanalItem>,
    tx_flows: TxFlowsAcquirer,
}

impl remote::RemoteHandle for RemoteHandle {
    fn send(&self, recipient: Addr, envelope: Envelope) -> remote::SendResult {
        let recipient = NetworkAddr::from_remote(recipient);

        match self.tx_flows.acquire(recipient) {
            Acquire::Done => {
                let mut item = Some(KanalItem::bounded(recipient, envelope));
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
        let recipient = NetworkAddr::from_remote(recipient);

        match self.tx_flows.try_acquire(recipient) {
            TryAcquire::Done => {
                let mut item = Some(KanalItem::bounded(recipient, envelope));
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

    fn unbounded_send(
        &self,
        recipient: Addr,
        envelope: Envelope,
    ) -> Result<(), SendError<Envelope>> {
        let recipient = NetworkAddr::from_remote(recipient);

        if likely(self.tx_flows.do_acquire(recipient)) {
            let mut item = Some(KanalItem::unbounded(recipient, envelope));
            match self.tx.try_send_option(&mut item) {
                Ok(true) => Ok(()),
                Ok(false) => unreachable!(),
                Err(_) => Err(SendError(item.take().unwrap().envelope.unwrap())),
            }
        } else {
            Err(SendError(envelope))
        }
    }

    fn respond(&self, token: ResponseToken, envelope: Result<Envelope, RequestError>) {
        debug_assert!(!token.is_forgotten());
        debug_assert!(token.sender().is_remote());

        let recipient = NetworkAddr::from_remote(token.sender());

        let mut item = Some(KanalItem {
            recipient,
            envelope,
            token: Some(token),
            bounded: false,
        });

        if likely(self.tx_flows.do_acquire(recipient)) {
            match self.tx.try_send_option(&mut item) {
                Ok(true) => return,
                Ok(false) => unreachable!(),
                Err(_) => {}
            }
        }

        if let Some(item) = item {
            let token = item.token.expect("response token set above");
            // Flow is closed, token is not required anymore
            token.forget();
        }

        trace!(addr = %recipient, "flow is closed, response is lost");
    }
}
