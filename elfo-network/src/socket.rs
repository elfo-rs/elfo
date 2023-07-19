use std::net::SocketAddr;

use bytes::{Buf, BufMut, Bytes, BytesMut};
use derive_more::Display;
use elfo_core::node::NodeNo;
use elfo_utils::likely;
use eyre::{eyre, Result, WrapErr};
use futures::Future;
use metrics::counter;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{
        tcp::{self, OwnedReadHalf, OwnedWriteHalf},
        TcpListener, TcpStream,
    },
};
use tracing::{debug, error, info, warn};

use crate::{
    codec::{encode::EncodeError, format::NetworkEnvelope},
    config::Transport,
    frame::{
        read::{FramedRead, FramedReadState, FramedReadStrategy},
        write::{FrameState, FramedWrite, FramedWriteStrategy},
    },
    node_map::{LaunchId, NodeInfo},
};

// === Socket ===

bitflags::bitflags! {
    #[derive(Clone, Copy)]
    pub struct Capabilities: u32 {
        const LZ4 = 0b01;
    }
}

const THIS_NODE_VERSION: u8 = 0;

pub(crate) struct Handshake {
    pub(crate) version: u8,
    pub(crate) node_no: NodeNo,
    pub(crate) launch_id: LaunchId,
    pub(crate) capabilities: Capabilities,
}

const HANDSHAKE_LENGTH: usize = 39;
const HANDSHAKE_MAGIC: u64 = 0xE1F0E1F0E1F0E1F0;
const HANDSHAKE_RESERVED_LENGTH: usize = 16;

impl Handshake {
    pub(crate) fn make_containing_buf() -> BytesMut {
        let mut buffer = BytesMut::new();
        buffer.resize(HANDSHAKE_LENGTH, 0);
        buffer
    }

    pub(crate) fn new(this_node: &NodeInfo, capabilities: Capabilities) -> Self {
        Self {
            version: THIS_NODE_VERSION,
            node_no: this_node.node_no,
            launch_id: this_node.launch_id,
            capabilities,
        }
    }

    pub(crate) fn as_bytes(&self) -> Bytes {
        let mut buf = BytesMut::with_capacity(HANDSHAKE_LENGTH);

        buf.put_u64_le(HANDSHAKE_MAGIC);
        buf.put_u8(self.version);
        buf.put_u16_le(self.node_no);
        buf.put_u64_le(self.launch_id.into());
        buf.put_u32_le(self.capabilities.bits());
        buf.put_slice(&[0; HANDSHAKE_RESERVED_LENGTH]);

        let result = buf.freeze();
        debug_assert!(result.len() == HANDSHAKE_LENGTH);
        result
    }

    pub(crate) fn from_bytes(bytes: &mut Bytes) -> Result<Self> {
        if bytes.len() < HANDSHAKE_LENGTH {
            return Err(eyre!(
                "expected handshake of length {}, got {} instead",
                HANDSHAKE_LENGTH,
                bytes.len()
            ));
        }

        if bytes.get_u64_le() != HANDSHAKE_MAGIC {
            return Err(eyre!("handshake magic did not match"));
        }

        let result = Self {
            version: bytes.get_u8(),
            node_no: bytes.get_u16_le(),
            launch_id: LaunchId::from_raw(bytes.get_u64_le()),
            capabilities: Capabilities::from_bits_truncate(bytes.get_u32_le()),
        };
        bytes.advance(HANDSHAKE_RESERVED_LENGTH);

        Ok(result)
    }
}

pub(crate) struct TcpSocket {
    read: OwnedReadHalf,
    write: OwnedWriteHalf,
    pub(crate) peer: Transport,
}

impl TcpSocket {
    fn new(stream: TcpStream, peer: Transport) -> Self {
        let (read, write) = stream.into_split();
        Self { read, write, peer }
    }

    pub(crate) async fn handshake(
        mut self,
        this_node: &NodeInfo,
        capabilities: Capabilities,
    ) -> Result<Option<Socket>> {
        let this_node_handshake = Handshake::new(this_node, capabilities);
        self.write
            .write_all_buf(&mut this_node_handshake.as_bytes())
            .await?;

        let mut buffer = Handshake::make_containing_buf();
        self.read.read_exact(&mut buffer).await?;
        let other_node_handshake = Handshake::from_bytes(&mut buffer.freeze())?;

        if this_node_handshake.node_no == other_node_handshake.node_no {
            return Ok(None);
        }

        let peer = Peer {
            node_no: other_node_handshake.node_no,
            launch_id: other_node_handshake.launch_id,
            transport: self.peer,
        };
        let version = this_node_handshake
            .version
            .min(other_node_handshake.version);
        let capabilities = this_node_handshake
            .capabilities
            .intersection(other_node_handshake.capabilities);

        Ok(Some(Socket::tcp(
            self.read,
            self.write,
            peer,
            version,
            capabilities,
        )))
    }
}

#[derive(Display, Clone)]
#[display(fmt = "peer(node_no={node_no}, launch_id={launch_id}, transport={transport})")]
pub(crate) struct Peer {
    pub(crate) node_no: NodeNo,
    pub(crate) launch_id: LaunchId,
    pub(crate) transport: Transport,
}

// TODO: Make `Socket`, `ReadHalf` and `WriteHalf` generic over transport type.
pub(crate) struct Socket {
    pub(crate) read: ReadHalf,
    pub(crate) write: WriteHalf,
    pub(crate) peer: Peer,
}

impl Socket {
    fn tcp(
        read: OwnedReadHalf,
        write: OwnedWriteHalf,
        peer: Peer,
        _version: u8,
        capabilities: Capabilities,
    ) -> Self {
        // TODO: maybe do something with the version.

        let (framed_read, framed_write) = if capabilities.contains(Capabilities::LZ4) {
            (FramedRead::lz4(), FramedWrite::lz4(None))
        } else {
            (FramedRead::none(), FramedWrite::none(None))
        };

        Self {
            read: ReadHalf::new(framed_read, read),
            write: WriteHalf::new(framed_write, write),
            peer,
        }
    }
}

pub(crate) struct ReadHalf {
    framing: FramedRead,
    read: tcp::OwnedReadHalf,
}

impl ReadHalf {
    pub(crate) fn new(framing: FramedRead, read: tcp::OwnedReadHalf) -> Self {
        Self { framing, read }
    }
}

impl ReadHalf {
    pub(crate) async fn recv(&mut self) -> Result<Option<NetworkEnvelope>> {
        let envelope = loop {
            let buffer = match self.framing.read()? {
                FramedReadState::NeedMoreData { buffer } => {
                    debug!(message = "framed read strategy requested more data");
                    buffer
                }
                FramedReadState::Done { decoded } => {
                    let (protocol, name) = decoded.payload.protocol_and_name();
                    debug!(
                        message = "framed read strategy decoded single envelope",
                        protocol, name,
                    );
                    // One of the envelopes inside the frame was decoded.
                    break decoded;
                }
            };

            let bytes_read = self.read.read(buffer).await?;
            if bytes_read == 0 {
                // EOF.
                return Ok(None);
            }

            self.framing.mark_filled(bytes_read);
            debug!(message = "read bytes from the socket", bytes_read);
        };

        let stats = self.framing.take_stats();
        counter!(
            "elfo_network_received_messages_total",
            stats.decode_stats.total_messages
        );
        counter!(
            "elfo_network_received_compressed_bytes_total",
            stats.decompress_stats.total_compressed_bytes
        );
        counter!(
            "elfo_network_received_decompressed_bytes_total",
            stats.decompress_stats.total_uncompressed_bytes
        );

        Ok(Some(envelope))
    }
}

pub(crate) struct WriteHalf {
    framing: FramedWrite,
    write: tcp::OwnedWriteHalf,
}

impl WriteHalf {
    pub(crate) fn new(framing: FramedWrite, write: tcp::OwnedWriteHalf) -> Self {
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
        let finalized = self.framing.finalize()?;
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

        debug!(message = "wrote bytes to socket", count = finalized.len());

        let stats = self.framing.take_stats();
        counter!(
            "elfo_network_sent_messages_total",
            stats.encode_stats.total_messages
        );
        counter!(
            "elfo_network_sent_decompressed_bytes_total",
            stats.compress_stats.total_uncompressed_bytes
        );
        counter!(
            "elfo_network_sent_compressed_bytes_total",
            stats.compress_stats.total_compressed_bytes
        );

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

// === connect ===

pub(crate) async fn connect(
    transport: &Transport,
    this_node: &NodeInfo,
    capabilities: Capabilities,
) -> Result<Option<Socket>> {
    match transport {
        Transport::Tcp(addr) => connect_tcp(*addr, this_node, capabilities).await,
    }
}

async fn connect_tcp(
    peer: SocketAddr,
    this_node: &NodeInfo,
    capabilities: Capabilities,
) -> Result<Option<Socket>> {
    // TODO: timeout
    // TODO: settings (Nagle's algorithm etc.)
    let stream = TcpStream::connect(peer).await?;
    stream.set_nodelay(true)?;
    let socket = TcpSocket::new(stream, Transport::Tcp(peer));
    socket.handshake(this_node, capabilities).await
}

// === listen ===

pub(crate) async fn listen(
    transport: &Transport,
    this_node: &NodeInfo,
    capabilities: Capabilities,
) -> Result<futures::stream::BoxStream<'static, Socket>> {
    match transport {
        Transport::Tcp(addr) => listen_tcp(*addr, this_node.clone(), capabilities).await,
    }
}

async fn listen_tcp(
    addr: SocketAddr,
    this_node: NodeInfo,
    capabilities: Capabilities,
) -> Result<futures::stream::BoxStream<'static, Socket>> {
    // TODO: timeout
    let listener = TcpListener::bind(addr)
        .await
        .wrap_err("cannot bind TCP listener")?;

    let accept = move |(listener, this_node): (TcpListener, NodeInfo)| async move {
        loop {
            match listener.accept().await {
                Ok((stream, peer)) => {
                    if let Err(err) = stream.set_nodelay(true) {
                        error!(
                            message = "cannot turn off Nagle's algorithm for incoming connection",
                            error = %err,
                            listener = %addr,
                        );
                        continue;
                    }
                    let socket = TcpSocket::new(stream, Transport::Tcp(peer));
                    match socket.handshake(&this_node, capabilities).await {
                        Ok(connection) => {
                            match connection {
                                Some(connection) => {
                                    return Some((connection, (listener, this_node)))
                                }
                                None => {
                                    info!(
                                        message = "connection to self ignored",
                                        listener = %addr,
                                    );

                                    // Continue listening.
                                }
                            }
                        }
                        Err(err) => {
                            warn!(
                                message = "handshake failed",
                                error = %err,
                                listener = %addr,
                            );

                            // Continue listening.
                        }
                    }
                }
                Err(err) => {
                    warn!(
                        message = "cannot accept TCP connection",
                        error = %err,
                        listener = %addr,
                    );

                    // Continue listening.
                }
            }
        }
    };

    Ok(Box::pin(futures::stream::unfold(
        (listener, this_node),
        accept,
    )))
}

#[cfg(test)]
mod tests {
    use elfo_core::{message, tracing::TraceId, Addr, Message};
    use futures::{future, stream::StreamExt};
    use std::{
        convert::TryFrom,
        net::{IpAddr, Ipv4Addr},
    };

    use crate::codec::format::NetworkEnvelopePayload;

    use super::*;

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

    async fn ensure_read_write(capabilities: Capabilities, port: u16) {
        let server_node = NodeInfo {
            node_no: 0,
            launch_id: LaunchId::generate(),
            groups: vec![],
        };
        let server_transport = Transport::Tcp(SocketAddr::new(
            IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),
            port,
        ));

        let mut listen_stream = listen(&server_transport, &server_node, capabilities)
            .await
            .expect("failed to bind server to a port");
        let server_socket_fut = listen_stream.next();

        let client_node = NodeInfo {
            node_no: 1,
            launch_id: LaunchId::generate(),
            groups: vec![],
        };
        let client_socket_fut = connect(&server_transport, &client_node, capabilities);

        let (server_socket, client_socket) =
            future::join(server_socket_fut, client_socket_fut).await;
        let mut server_socket = server_socket.expect("server failed");
        let mut client_socket = client_socket
            .expect("failed to connect to the server")
            .expect("handshake failed");

        for i in 0..10 {
            let envelope = NetworkEnvelope {
                sender: Addr::NULL,
                recipient: Addr::NULL,
                trace_id: TraceId::try_from(1).unwrap(),
                payload: NetworkEnvelopePayload::Regular {
                    message: TestSocketMessage("a".repeat(i * 10)).upcast(),
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
    async fn read_write_no_framing() {
        ensure_read_write(Capabilities::empty(), 9200).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[tracing_test::traced_test]
    async fn read_write_lz4() {
        ensure_read_write(Capabilities::LZ4, 9201).await;
    }
}
