use std::{future::Future, pin::Pin, task, time::Duration};

use derive_more::{Constructor, Display};
use eyre::{eyre, Result, WrapErr};
use futures::{stream::BoxStream, StreamExt};
use metrics::{counter, histogram};
use pin_project::pin_project;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tracing::{trace, warn};

use elfo_core::addr::{NodeLaunchId, NodeNo};
use elfo_utils::{likely, time::Instant};

use self::idleness::{IdleTrack, IdleTracker};
use crate::{
    codec::{decode::EnvelopeDetails, encode::EncodeError, format::NetworkEnvelope},
    config::Transport,
    frame::{
        read::{FramedRead, FramedReadState, FramedReadStrategy},
        write::{FrameState, FramedWrite, FramedWriteStrategy},
    },
};

pub(crate) use self::{
    capabilities::{
        compression::{Algorithms, Compression},
        Capabilities,
    },
    raw::SocketInfo,
};

mod capabilities;
mod handshake;
mod idleness;
mod raw;

pub(crate) struct Socket {
    pub(crate) info: SocketInfo,
    pub(crate) peer: Peer,
    pub(crate) capabilities: Capabilities,
    pub(crate) read: ReadHalf,
    pub(crate) write: WriteHalf,
    pub(crate) idle: IdleTracker,
}

#[derive(Debug, Display, Clone, Copy, Constructor, PartialEq, Eq)]
#[display("peer(node_no={node_no}, launch_id={launch_id})")] // TODO: use `valuable` after tracing#1570
pub(crate) struct Peer {
    pub(crate) node_no: NodeNo,
    pub(crate) launch_id: NodeLaunchId,
}

impl Socket {
    fn new(raw: raw::Socket, handshake: handshake::Handshake) -> Self {
        // The actual algorithms selection logic happens in
        // [`Compression::intersection`], so we just choose the algorithm from
        // preferred.
        let algos = handshake.capabilities.compression().preferred();

        let (framed_read, framed_write) = if algos.contains(Algorithms::LZ4) {
            (FramedRead::lz4(), FramedWrite::lz4(None))
        } else {
            (FramedRead::none(), FramedWrite::none(None))
        };
        let (idle_tracker, idle_track) = IdleTracker::new();

        Self {
            info: raw.info,
            peer: Peer::new(handshake.node_no, handshake.launch_id),
            capabilities: handshake.capabilities,
            read: ReadHalf::new(framed_read, raw.read, idle_track),
            write: WriteHalf::new(framed_write, raw.write),
            idle: idle_tracker,
        }
    }
}

pub(crate) struct ReadHalf {
    framing: FramedRead,
    read: raw::OwnedReadHalf,
    idle: IdleTrack,
}

#[derive(Debug)]
pub(crate) enum ReadError {
    EnvelopeSkipped(EnvelopeDetails),
    Fatal(eyre::Report),
}

impl<T> From<T> for ReadError
where
    T: Into<eyre::Report>,
{
    fn from(error: T) -> Self {
        Self::Fatal(error.into())
    }
}

impl ReadHalf {
    fn new(framing: FramedRead, read: raw::OwnedReadHalf, idle: IdleTrack) -> Self {
        Self {
            framing,
            read,
            idle,
        }
    }

    fn report_framing_metrics(&mut self) {
        let stats = self.framing.take_stats();
        counter!(
            "elfo_network_received_messages_total",
            stats.decode_stats.total_messages_decoded
                + stats.decode_stats.total_messages_decoding_skipped
        );
        counter!(
            "elfo_network_received_uncompressed_bytes_total",
            stats.decompress_stats.total_uncompressed_bytes
        );
    }

    pub(crate) async fn recv(&mut self) -> Result<Option<NetworkEnvelope>, ReadError> {
        let envelope = loop {
            let buffer = match self.framing.read()? {
                FramedReadState::NeedMoreData { buffer } => {
                    trace!("framed read strategy requested more data");
                    buffer
                }
                FramedReadState::EnvelopeSkipped(details) => {
                    self.idle.update();
                    return Err(ReadError::EnvelopeSkipped(details));
                }
                FramedReadState::Done { decoded } => {
                    self.idle.update();
                    let (protocol, name) = decoded.payload.protocol_and_name();
                    trace!(
                        message = "framed read strategy decoded single envelope",
                        protocol,
                        name,
                    );
                    // One of the envelopes inside the frame was decoded.
                    break decoded;
                }
            };

            let bytes_read = self.read.read(buffer).await?;

            // Large messages cannot be read in a single `read()` call, so we should
            // additionally update the idle tracker even without waiting until the
            // message is fully decoded to prevent false positive disconnects.
            self.idle.update();

            if bytes_read == 0 {
                // EOF.
                return Ok(None);
            }
            counter!("elfo_network_received_bytes_total", bytes_read as u64);
            self.report_framing_metrics();

            self.framing.mark_filled(bytes_read);
            trace!(message = "read bytes from the socket", bytes_read);
        };

        self.report_framing_metrics();

        Ok(Some(envelope))
    }
}

pub(crate) struct WriteHalf {
    framing: FramedWrite,
    write: raw::OwnedWriteHalf,
}

impl WriteHalf {
    fn new(framing: FramedWrite, write: raw::OwnedWriteHalf) -> Self {
        Self { framing, write }
    }

    /// Encodes the message into the internal buffer.
    ///
    /// Returns
    /// * `Ok(Some(FrameState))` if the message is added successfully.
    /// * `Ok(None)` if the message is skipped because of encoding errors.
    /// * `Err(err)` if an unrecoverable error happened.
    pub(crate) fn feed(&mut self, envelope: &NetworkEnvelope) -> Result<Option<FrameState>> {
        // TODO: timeout, it should be clever
        // TODO: we should also emit metrics here, not only in `flush()`.
        let write_result = self.framing.write(envelope);
        match write_result {
            Ok(state) => Ok(Some(state)),
            Err(EncodeError::Skipped) => Ok(None),
            Err(EncodeError::Fatal(err)) => Err(err.into()),
        }
    }

    /// Flushed the internal buffer unconditionally.
    pub(crate) async fn flush(&mut self) -> Result<()> {
        self.do_flush().measure_iowait().await
    }

    async fn do_flush(&mut self) -> Result<()> {
        let finalized = self.framing.finalize()?;
        let finalized_len = finalized.len();

        let mut result = self
            .write
            .write_all(finalized)
            .await
            .context("failed to write frame");

        if likely(result.is_ok()) {
            result = self
                .write
                .flush()
                .await
                .context("failed to flush the frame");
        }

        let stats = self.framing.take_stats();
        let mut total_messages_sent = stats.encode_stats.total_messages_encoding_skipped;

        if likely(result.is_ok()) {
            trace!(message = "wrote bytes to socket", count = finalized_len);

            counter!("elfo_network_sent_bytes_total", finalized_len as u64);
            counter!(
                "elfo_network_sent_uncompressed_bytes_total",
                stats.compress_stats.total_uncompressed_bytes
            );

            total_messages_sent += stats.encode_stats.total_messages_encoded;
        }

        counter!("elfo_network_sent_messages_total", total_messages_sent);

        result
    }

    // Encodes the message and flushes the internal buffer.
    pub(crate) fn send<'a>(
        &'a mut self,
        envelope: &NetworkEnvelope,
    ) -> impl Future<Output = Result<()>> + 'a + Send {
        let result = self.feed(envelope);
        async move {
            result
                .wrap_err("fatal serialization error")?
                .ok_or(eyre!("non-fatal serialization error"))?;
            self.flush().await
        }
    }
}

// TODO: make configurable.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const LISTEN_TIMEOUT: Duration = Duration::from_secs(3);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
const HANDSHAKE_CONCURRENCY: usize = 64;

pub(crate) async fn connect(
    addr: &Transport,
    node_no: NodeNo,
    launch_id: NodeLaunchId,
    capabilities: Capabilities,
) -> Result<Socket> {
    let mut raw_socket = timeout(CONNECT_TIMEOUT, raw::connect(addr)).await?;
    let handshaking = handshake::handshake(&mut raw_socket, node_no, launch_id, capabilities);
    let handshake = timeout(HANDSHAKE_TIMEOUT, handshaking)
        .await
        .wrap_err("handshake")?;
    Ok(Socket::new(raw_socket, handshake))
}

pub(crate) async fn listen(
    addr: &Transport,
    node_no: NodeNo,
    launch_id: NodeLaunchId,
    capabilities: Capabilities,
) -> Result<BoxStream<'static, Socket>> {
    let stream = timeout(LISTEN_TIMEOUT, raw::listen(addr)).await?;
    let stream = stream
        .map(move |mut raw_socket| async move {
            let handshaking =
                handshake::handshake(&mut raw_socket, node_no, launch_id, capabilities);

            match timeout(HANDSHAKE_TIMEOUT, handshaking).await {
                Ok(handshake) => Some(Socket::new(raw_socket, handshake)),
                Err(err) => {
                    warn!(
                        message = "cannot handshake accepted connection",
                        error = %err,
                        socket = %raw_socket.info,
                    );
                    None
                }
            }
        })
        // Enable concurrent handshakes.
        .buffer_unordered(HANDSHAKE_CONCURRENCY)
        .filter_map(|opt| async move { opt });

    Ok(Box::pin(stream))
}

async fn timeout<T>(duration: Duration, fut: impl Future<Output = Result<T>>) -> Result<T> {
    tokio::time::timeout(duration, fut).await?
}

trait FutureExt: Future + Sized {
    // Measures the time spent in `Pending` state as IO wait time.
    fn measure_iowait(self) -> MeasureIowait<Self> {
        MeasureIowait {
            inner: self,
            idle_since: None,
        }
    }
}

impl<F: Future> FutureExt for F {}

#[pin_project]
struct MeasureIowait<F> {
    #[pin]
    inner: F,
    idle_since: Option<Instant>,
}

impl<F: Future> Future for MeasureIowait<F> {
    type Output = F::Output;

    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> task::Poll<Self::Output> {
        let this = self.project();
        let result = this.inner.poll(cx);

        if result.is_pending() {
            this.idle_since.get_or_insert_with(Instant::now);
        } else if let Some(start) = this.idle_since.take() {
            let elapsed = start.elapsed_secs_f64();
            histogram!("elfo_network_io_write_waiting_time_seconds", elapsed);
        }

        result
    }
}

#[cfg(test)]
mod tests {
    use std::convert::TryFrom;

    use futures::{future, stream::StreamExt};
    use tracing::debug;

    use elfo_core::{_priv::AnyMessage, message, tracing::TraceId};

    use crate::codec::format::{NetworkAddr, NetworkEnvelopePayload};

    use super::*;

    const EMPTY_CAPS: Capabilities = Capabilities::from_bits(0);

    #[message]
    #[derive(PartialEq)]
    struct TestSocketMessage(String);

    fn feed_frame(client_socket: &mut Socket, envelope: &NetworkEnvelope) {
        for _ in 0..100 {
            client_socket
                .write
                .feed(envelope)
                .expect("failed to feed message");
        }
    }

    async fn read_frame(server_socket: &mut Socket, sent_envelope: &NetworkEnvelope) {
        for i in 0..100 {
            debug!("Decoding envelope #{}", i);
            let recv_envelope = server_socket
                .read
                .recv()
                .await
                .expect("error receiving the message")
                .expect("unexpected EOF");
            assert_eq!(recv_envelope.recipient, sent_envelope.recipient);
            assert_eq!(recv_envelope.sender, sent_envelope.sender);
            assert_eq!(recv_envelope.trace_id, sent_envelope.trace_id);

            if let NetworkEnvelopePayload::Regular {
                message: recv_message,
            } = recv_envelope.payload
            {
                if let NetworkEnvelopePayload::Regular {
                    message: sent_message,
                } = &sent_envelope.payload
                {
                    assert_eq!(
                        recv_message.downcast_ref::<TestSocketMessage>().unwrap(),
                        sent_message.downcast_ref::<TestSocketMessage>().unwrap(),
                    );
                } else {
                    panic!("unexpected kind of the sent message");
                }
            } else {
                panic!("unexpected kind of the received message");
            }
        }
    }

    fn spawn_flush(mut client_socket: Socket) -> tokio::task::JoinHandle<Socket> {
        tokio::spawn(async move {
            client_socket.write.flush().await.expect("failed to flush");
            client_socket
        })
    }

    async fn ensure_read_write(transport: &str, capabilities: Capabilities) {
        let transport = transport.parse().unwrap();
        let node_no = NodeNo::from_bits(2).unwrap();
        let launch_id = NodeLaunchId::from_bits(1).unwrap();

        let mut listen_stream = listen(&transport, node_no, launch_id, capabilities)
            .await
            .expect("failed to bind server to a port");
        let server_socket_fut = listen_stream.next();

        let node_no = NodeNo::from_bits(1).unwrap();
        let launch_id = NodeLaunchId::from_bits(2).unwrap();
        let client_socket_fut = connect(&transport, node_no, launch_id, capabilities);

        let (server_socket, client_socket) =
            future::join(server_socket_fut, client_socket_fut).await;
        let mut server_socket = server_socket.expect("server failed");
        let mut client_socket = client_socket.expect("failed to connect to the server");

        for i in 0..10 {
            let envelope = NetworkEnvelope {
                sender: NetworkAddr::NULL,
                recipient: NetworkAddr::NULL,
                trace_id: TraceId::try_from(1).unwrap(),
                payload: NetworkEnvelopePayload::Regular {
                    message: AnyMessage::new(TestSocketMessage("a".repeat(i * 10))),
                },
            };

            feed_frame(&mut client_socket, &envelope);
            let flush_handle = spawn_flush(client_socket);
            read_frame(&mut server_socket, &envelope).await;
            client_socket = flush_handle.await.expect("flush panicked");
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[tracing_test::traced_test]
    async fn tcp_read_write_no_framing() {
        ensure_read_write("tcp://127.0.0.1:9200", EMPTY_CAPS).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[tracing_test::traced_test]
    async fn tcp_read_write_lz4() {
        ensure_read_write(
            "tcp://127.0.0.1:9201",
            Capabilities::new(Compression::new(Algorithms::empty(), Algorithms::LZ4)),
        )
        .await;
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[tracing_test::traced_test]
    async fn uds_read_write_no_framing() {
        ensure_read_write("uds://test_uds_read_write_no_framing.socket", EMPTY_CAPS).await;
    }
}
