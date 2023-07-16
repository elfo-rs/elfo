use std::net::SocketAddr;

use eyre::{Result, WrapErr};
use futures::{SinkExt, StreamExt};
use metrics::counter;
use tokio::net::{tcp, TcpListener, TcpStream};
use tokio_util::codec::{FramedRead, FramedWrite};
use tracing::warn;

use crate::{
    codec::{Decoder, EncodeError, Encoder, NetworkEnvelope},
    config::Transport,
};

// === Socket ===

// TODO: versioning, compression settings etc.

pub(crate) struct Socket {
    pub(crate) read: ReadHalf,
    pub(crate) write: WriteHalf,
    pub(crate) peer: Transport,
}

impl Socket {
    fn tcp(stream: TcpStream, peer: Transport) -> Self {
        let (rx, tx) = stream.into_split();
        let read = FramedRead::new(rx, Decoder::new());
        let mut write = FramedWrite::new(tx, Encoder::new());

        // TODO: how should it work with compression?
        write.set_backpressure_boundary(128 * 1024);

        Self {
            read: ReadHalf(read),
            write: WriteHalf(write),
            peer,
        }
    }
}

pub(crate) struct ReadHalf(FramedRead<tcp::OwnedReadHalf, Decoder>);

impl ReadHalf {
    pub(crate) async fn recv(&mut self) -> Result<Option<NetworkEnvelope>> {
        let result = self.0.next().await.transpose();

        let stats = self.0.decoder_mut().take_stats();
        counter!("elfo_network_received_messages_total", stats.messages);
        counter!(
            "elfo_network_received_uncompressed_bytes_total",
            stats.bytes
        );
        // TODO: elfo_network_received_compressed_bytes_total

        result
    }
}

pub(crate) struct WriteHalf(FramedWrite<tcp::OwnedWriteHalf, Encoder>);

impl WriteHalf {
    /// Encodes the message and flushes the internal buffer if needed.
    ///
    /// Returns
    /// * `Ok(true)` if the message is added successfully.
    /// * `Ok(false)` if the message is skipped because of encoding errors.
    /// * `Err(err)` if an unrecoverable error happened.
    // TODO: it would be nice to have only `&NetworkEnvelope` here.
    // It requires either to replace `FramedWrite` or make `Message: Sync`.
    pub(crate) async fn feed(&mut self, envelope: NetworkEnvelope) -> Result<bool> {
        // TODO: timeout, it should be clever
        // TODO: we should also emit metrics here, not only in `flush()`.
        match self.0.feed(envelope).await {
            Ok(()) => Ok(true),
            Err(EncodeError::Skipped) => Ok(false),
            Err(EncodeError::Fatal(err)) => Err(err.into()),
        }
    }

    /// Flushed the internal buffer unconditionally.
    pub(crate) async fn flush(&mut self) -> Result<()> {
        let result = self.0.flush().await;

        let stats = self.0.encoder_mut().take_stats();
        counter!("elfo_network_sent_messages_total", stats.messages);
        counter!("elfo_network_sent_uncompressed_bytes_total", stats.bytes);
        // TODO: elfo_network_sent_compressed_bytes_total

        if let Err(EncodeError::Fatal(err)) = result {
            return Err(err.into());
        }

        Ok(())
    }

    // Encodes the message and flushes the internal buffer.
    pub(crate) async fn send(&mut self, envelope: NetworkEnvelope) -> Result<()> {
        self.feed(envelope).await?;
        self.flush().await
    }
}

// === connect ===

pub(crate) async fn connect(transport: &Transport) -> Result<Socket> {
    match transport {
        Transport::Tcp(addr) => connect_tcp(*addr).await,
    }
}

async fn connect_tcp(peer: SocketAddr) -> Result<Socket> {
    // TODO: timeout
    // TODO: settings (keepalive, linger, etc.)
    let stream = TcpStream::connect(peer).await?;
    let _ = stream.set_nodelay(true);
    Ok(Socket::tcp(stream, Transport::Tcp(peer)))
}

// === listen ===

pub(crate) async fn listen(
    transport: &Transport,
) -> Result<futures::stream::BoxStream<'static, Socket>> {
    match transport {
        Transport::Tcp(addr) => listen_tcp(*addr).await,
    }
}

async fn listen_tcp(addr: SocketAddr) -> Result<futures::stream::BoxStream<'static, Socket>> {
    // TODO: timeout
    let listener = TcpListener::bind(addr)
        .await
        .wrap_err("cannot bind TCP listener")?;

    let accept = move |listener: TcpListener| async move {
        loop {
            match listener.accept().await {
                Ok((stream, peer)) => {
                    let _ = stream.set_nodelay(true);
                    let connection = Socket::tcp(stream, Transport::Tcp(peer));
                    return Some((connection, listener));
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

    Ok(Box::pin(futures::stream::unfold(listener, accept)))
}
