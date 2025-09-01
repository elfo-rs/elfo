use std::net::{IpAddr, SocketAddr};

use derive_more::Display;
use eyre::{Result, WrapErr};
use futures::Stream;
use tracing::warn;
use turmoil06::net::{TcpListener, TcpStream};

pub(super) use turmoil06::net::tcp::{OwnedReadHalf, OwnedWriteHalf};

const DEFAULT_PORT: u16 = 0xE1F0;

#[derive(Clone, Display, PartialEq, Eq)]
#[display("turmoil06(local={local}, peer={peer})")] // TODO: use `valuable` after tracing#1570
pub(crate) struct SocketInfo {
    local: String,
    peer: String,
}

pub(super) struct Socket {
    pub(super) read: OwnedReadHalf,
    pub(super) write: OwnedWriteHalf,
    pub(super) info: SocketInfo,
}

fn prepare_stream(stream: TcpStream) -> Result<Socket> {
    let info = SocketInfo {
        local: stringify_addr(stream.local_addr().wrap_err("cannot get local addr")?),
        peer: stringify_addr(stream.peer_addr().wrap_err("cannot get peer addr")?),
    };

    let (read, write) = stream.into_split();
    Ok(Socket { read, write, info })
}

fn stringify_addr(addr: SocketAddr) -> String {
    let (ip, port) = (addr.ip(), addr.port());
    let host = turmoil06::reverse_lookup(ip).unwrap_or_else(|| ip.to_string());
    format!("{host}:{port}")
}

pub(super) async fn connect(addr: &str) -> Result<Socket> {
    prepare_stream(TcpStream::connect(prepare_addr(addr)).await?)
}

pub(super) async fn listen(addr: &str) -> Result<impl Stream<Item = Socket> + 'static> {
    let listener = TcpListener::bind(prepare_addr(addr)).await?;

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
                        message = "cannot accept turmoil connection",
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

fn prepare_addr(addr: &str) -> String {
    if addr.parse::<SocketAddr>().is_ok() {
        return addr.to_string();
    }

    if addr.parse::<IpAddr>().is_ok() {
        return format!("{addr}:{DEFAULT_PORT}");
    }

    if addr.contains(':') {
        addr.to_string()
    } else {
        format!("{addr}:{DEFAULT_PORT}")
    }
}
