use std::net::SocketAddr;

use eyre::{Result, WrapErr};
use futures::{SinkExt, StreamExt};
use tokio::net::{tcp, TcpListener, TcpStream};
use tokio_util::codec::{FramedRead, FramedWrite};
use tracing::warn;

use elfo_core::{Addr, Envelope};

use crate::{
    codec::{Decoder, Encoder},
    config::Transport,
};

const MAX_FRAME_SIZE: u32 = 65536; // TODO: make it configurable.

// === Connection ===

// TODO: versioning, compression settings etc.

pub(crate) struct Connection {
    pub(crate) read: ReadHalf,
    pub(crate) write: WriteHalf,
    pub(crate) peer: Transport,
}

impl Connection {
    fn tcp(stream: TcpStream, peer: Transport) -> Self {
        let (rx, tx) = stream.into_split();
        Self {
            read: ReadHalf(FramedRead::new(rx, Decoder::new(MAX_FRAME_SIZE))),
            write: WriteHalf(FramedWrite::new(tx, Encoder::new(MAX_FRAME_SIZE))),
            peer: peer.into(),
        }
    }
}

pub(crate) struct ReadHalf(FramedRead<tcp::OwnedReadHalf, Decoder>);

impl ReadHalf {
    pub(crate) async fn recv(&mut self) -> Result<Option<(Envelope, Addr)>> {
        self.0.next().await.transpose()
    }
}

pub(crate) struct WriteHalf(FramedWrite<tcp::OwnedWriteHalf, Encoder>);

impl WriteHalf {
    pub(crate) async fn send(&mut self, envelope: Envelope, addr: Addr) -> Result<()> {
        // TODO: timeout, it should be clever
        self.0.send((envelope, addr)).await?;
        Ok(())
    }

    pub(crate) async fn flush(&mut self) -> Result<()> {
        self.0.flush().await?;
        Ok(())
    }

    pub(crate) async fn send_flush(&mut self, envelope: Envelope, addr: Addr) -> Result<()> {
        self.send(envelope, addr).await?;
        self.flush().await?;
        Ok(())
    }
}

// === connect ===

pub(crate) async fn connect(transport: &Transport) -> Result<Connection> {
    match transport {
        Transport::Tcp(addr) => connect_tcp(*addr).await,
    }
}

async fn connect_tcp(peer: SocketAddr) -> Result<Connection> {
    // TODO: timeout
    // TODO: settings (Nagle's algorithm etc.)
    let stream = TcpStream::connect(peer).await?;
    Ok(Connection::tcp(stream, Transport::Tcp(peer)))
}

// === listen ===

pub(crate) async fn listen(
    transport: &Transport,
) -> Result<futures::stream::BoxStream<'static, Connection>> {
    match transport {
        Transport::Tcp(addr) => listen_tcp(*addr).await,
    }
}

async fn listen_tcp(addr: SocketAddr) -> Result<futures::stream::BoxStream<'static, Connection>> {
    // TODO: timeout
    let listener = TcpListener::bind(addr)
        .await
        .wrap_err("cannot bind TCP listener")?;

    let accept = move |listener: TcpListener| async move {
        loop {
            match listener.accept().await {
                Ok((stream, peer)) => {
                    let connection = Connection::tcp(stream, Transport::Tcp(peer));
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
