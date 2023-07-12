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
use tracing::{error, info, warn};

use crate::{
    codec::{EncodeError, NetworkEnvelope},
    codec_direct::DecodeState,
    config::Transport,
    frame::{
        read::{FramedRead, FramedReadStrategy},
        write::{FrameState, FramedWrite, FramedWriteStrategy},
    },
    node_map::{LaunchId, NodeInfo},
};

// === Socket ===

// TODO: versioning, compression settings etc.

bitflags::bitflags! {
    pub struct Capabilities: u32 {
        const LZ4 = 0b01;
    }
}

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

    pub(crate) fn new(this_node: &NodeInfo) -> Self {
        // TODO laplab: fill version and capabilities with meaningful values.
        Self {
            version: 0,
            node_no: this_node.node_no,
            launch_id: this_node.launch_id,
            capabilities: Capabilities::empty(),
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

    pub(crate) async fn handshake(mut self, this_node: &NodeInfo) -> Result<Option<Socket>> {
        let this_node_handshake = Handshake::new(this_node);
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
        _capabilities: Capabilities,
    ) -> Self {
        // TODO: maybe do something with the version.
        // TODO: use capabilities to switch between framing strategies.
        Self {
            read: ReadHalf::new(FramedRead::lz4(), read),
            write: WriteHalf {
                framing: FramedWrite::lz4(None),
                write,
            },
            peer,
        }
    }
}

pub(crate) struct ReadHalf {
    framing: FramedRead,
    buffer: Vec<u8>,
    position: usize,
    read: tcp::OwnedReadHalf,
}

const BUFFER_INITIAL_CAPACITY: usize = 8192;
const BUFFER_CHUNK_SIZE: usize = 512;

impl ReadHalf {
    pub(crate) fn new(framing: FramedRead, read: tcp::OwnedReadHalf) -> Self {
        Self {
            framing,
            buffer: Vec::with_capacity(BUFFER_INITIAL_CAPACITY),
            position: 0,
            read,
        }
    }
}

impl ReadHalf {
    pub(crate) async fn recv(&mut self) -> Result<Option<NetworkEnvelope>> {
        let envelope = loop {
            let length_estimate = match self.framing.read(&self.buffer[self.position..])? {
                DecodeState::NeedMoreData { length_estimate } => {
                    error!(message = "laplab: read half requested more data", min = %length_estimate);
                    length_estimate
                }
                DecodeState::Done {
                    bytes_consumed,
                    decoded: None,
                } => {
                    // The frame was fully decoded, so we remove now unused bytes at the beginning
                    // of the buffer.
                    self.position += bytes_consumed;
                    self.buffer.drain(..self.position);
                    error!(
                        message = "laplab: frame consumed",
                        last_pos = self.position,
                        was = (self.position + self.buffer.len()),
                        remaining = self.buffer.len()
                    );
                    self.position = 0;
                    // We now need to ask the decoder once more for the length estimate.
                    continue;
                }
                DecodeState::Done {
                    bytes_consumed,
                    decoded: Some(envelope),
                } => {
                    // One of the envelopes inside the frame was decoded.
                    error!(
                        message = "laplab: read half got an envelope",
                        envelope = format!("{:?}", envelope)
                    );
                    self.position += bytes_consumed;
                    break envelope;
                }
            };

            // Decoder requested for more data to be read. We round up the number of
            // requested bytes to be a multiple of the chunk size to avoid
            // reading too few bytes per syscall.
            let old_buffer_size = self.buffer.len();
            debug_assert!(length_estimate > old_buffer_size);
            let new_buffer_size =
                ((length_estimate + BUFFER_CHUNK_SIZE - 1) / BUFFER_CHUNK_SIZE) * BUFFER_CHUNK_SIZE;
            self.buffer.resize(new_buffer_size, 0);

            // We try to read the whole chunk, but as soon as we have enough for the decoder
            // to work with, we stop.
            let mut buffer_position = old_buffer_size;
            while buffer_position < length_estimate {
                let bytes_read = self.read.read(&mut self.buffer[buffer_position..]).await?;
                if bytes_read == 0 {
                    // EOF.
                    return Ok(None);
                }
                buffer_position += bytes_read;
            }
            error!(
                message = "laplab: read half read bytes",
                count = (buffer_position - old_buffer_size)
            );
            self.buffer.resize(buffer_position, 0);
        };

        let stats = self.framing.take_stats();
        counter!("elfo_network_received_messages_total", stats.messages);
        counter!(
            "elfo_network_received_uncompressed_bytes_total",
            stats.bytes
        );
        // TODO: elfo_network_received_compressed_bytes_total

        Ok(Some(envelope))
    }
}

pub(crate) struct WriteHalf {
    framing: FramedWrite,
    write: tcp::OwnedWriteHalf,
}

impl WriteHalf {
    /// Encodes the message and flushes the internal buffer if needed.
    ///
    /// Returns
    /// * `Ok(true)` if the message is added successfully.
    /// * `Ok(false)` if the message is skipped because of encoding errors.
    /// * `Err(err)` if an unrecoverable error happened.
    pub(crate) fn feed<'a>(
        &'a mut self,
        envelope: &NetworkEnvelope,
    ) -> impl Future<Output = Result<bool>> + 'a + Send {
        // TODO: timeout, it should be clever
        // TODO: we should also emit metrics here, not only in `flush()`.
        let write_result = self.framing.write(envelope);
        async move {
            let state = match write_result {
                Ok(state) => state,
                Err(EncodeError::Skipped) => return Ok(false),
                Err(EncodeError::Fatal(err)) => return Err(err.into()),
            };

            if matches!(state, FrameState::FlushAdvised) {
                self.flush().await?;
            }
            Ok(true)
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

        error!(
            message = "laplab: write half sent bytes",
            count = finalized.len()
        );

        let stats = self.framing.take_stats();
        counter!("elfo_network_sent_messages_total", stats.messages);
        counter!("elfo_network_sent_uncompressed_bytes_total", stats.bytes);
        // TODO: elfo_network_sent_compressed_bytes_total

        result
    }

    // Encodes the message and flushes the internal buffer.
    pub(crate) fn send<'a>(
        &'a mut self,
        envelope: &NetworkEnvelope,
    ) -> impl Future<Output = Result<()>> + 'a + Send {
        // NOTE: We are not used `feed()` here to avoid double flushing.
        let result = self
            .framing
            .write(envelope)
            .map_err(|_| eyre!("failed to serialize envelope"));
        async move {
            result?;
            self.flush().await
        }
    }
}

// === connect ===

pub(crate) async fn connect(transport: &Transport, this_node: &NodeInfo) -> Result<Option<Socket>> {
    match transport {
        Transport::Tcp(addr) => connect_tcp(*addr, this_node).await,
    }
}

async fn connect_tcp(peer: SocketAddr, this_node: &NodeInfo) -> Result<Option<Socket>> {
    // TODO: timeout
    // TODO: settings (Nagle's algorithm etc.)
    let stream = TcpStream::connect(peer).await?;
    let socket = TcpSocket::new(stream, Transport::Tcp(peer));
    socket.handshake(this_node).await
}

// === listen ===

pub(crate) async fn listen(
    transport: &Transport,
    this_node: &NodeInfo,
) -> Result<futures::stream::BoxStream<'static, Socket>> {
    match transport {
        Transport::Tcp(addr) => listen_tcp(*addr, this_node.clone()).await,
    }
}

async fn listen_tcp(
    addr: SocketAddr,
    this_node: NodeInfo,
) -> Result<futures::stream::BoxStream<'static, Socket>> {
    // TODO: timeout
    let listener = TcpListener::bind(addr)
        .await
        .wrap_err("cannot bind TCP listener")?;

    let accept = move |(listener, this_node): (TcpListener, NodeInfo)| async move {
        loop {
            match listener.accept().await {
                Ok((stream, peer)) => {
                    let socket = TcpSocket::new(stream, Transport::Tcp(peer));
                    match socket.handshake(&this_node).await {
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
