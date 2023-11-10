use std::net::SocketAddr;

use derive_more::Display;
use eyre::{Result, WrapErr};
use futures::Stream;
use tokio::net::{TcpListener, TcpStream};
use tracing::warn;

pub(super) use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};

#[derive(Clone, Display)]
#[display(fmt = "tcp(local={local}, peer={peer})")] // TODO: use `valuable` after tracing#1570
pub(crate) struct SocketInfo {
    local: SocketAddr,
    peer: SocketAddr,
}

pub(super) struct Socket {
    pub(super) read: OwnedReadHalf,
    pub(super) write: OwnedWriteHalf,
    pub(super) info: SocketInfo,
}

fn prepare_stream(stream: TcpStream) -> Result<Socket> {
    let info = SocketInfo {
        local: stream.local_addr().wrap_err("cannot get local addr")?,
        peer: stream.peer_addr().wrap_err("cannot get peer addr")?,
    };

    // TODO: settings (keepalive, linger, etc.)
    if let Err(err) = stream.set_nodelay(true) {
        warn!(
            message = "cannot turn off Nagle's algorithm",
            reason = %err,
            socket = %info,
        );
    }

    let (read, write) = stream.into_split();
    Ok(Socket { read, write, info })
}

pub(super) async fn connect(addr: &str) -> Result<Socket> {
    prepare_stream(TcpStream::connect(addr).await?)
}

pub(super) async fn listen(addr: &str) -> Result<impl Stream<Item = Socket> + 'static> {
    let listener = TcpListener::bind(addr).await?;

    let accept = move |listener: TcpListener| async move {
        loop {
            let result = listener
                .accept()
                .await
                .map_err(Into::into)
                .and_then(|(socket, _)| prepare_stream(socket));

            match result {
                Ok(socket) => return Some((socket, listener)),
                Err(err) => {
                    warn!(
                        message = "cannot accept TCP connection",
                        error = %err,
                        // TODO: addr
                    );

                    // Continue listening.
                }
            }
        }
    };

    Ok(futures::stream::unfold(listener, accept))
}
