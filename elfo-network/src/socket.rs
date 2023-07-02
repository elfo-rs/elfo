use std::net::SocketAddr;

use eyre::{Result, WrapErr};
use futures::{SinkExt, StreamExt};
use tokio::net::{tcp, TcpListener, TcpStream};
use tokio_util::codec::{FramedRead, FramedWrite};
use tracing::warn;

use crate::{
    codec::{Decoder, Encoder, NetworkEnvelope},
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
        self.0.next().await.transpose()
    }
}

pub(crate) struct WriteHalf(FramedWrite<tcp::OwnedWriteHalf, Encoder>);

impl WriteHalf {
    // TODO: it would be nice to have only `&NetworkEnvelope` here.
    // It requires either to replace `FramedWrite` or make `Message: Sync`.
    pub(crate) async fn send(&mut self, envelope: NetworkEnvelope) -> Result<()> {
        // TODO: timeout, it should be clever
        self.0.send(envelope).await?;
        Ok(())
    }

    pub(crate) async fn flush(&mut self) -> Result<()> {
        self.0.flush().await?;
        Ok(())
    }

    pub(crate) async fn send_flush(&mut self, envelope: NetworkEnvelope) -> Result<()> {
        self.send(envelope).await?;
        self.flush().await?;
        Ok(())
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
    // TODO: settings (Nagle's algorithm etc.)
    let stream = TcpStream::connect(peer).await?;
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
