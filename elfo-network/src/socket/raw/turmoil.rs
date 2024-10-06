use std::net::SocketAddr;

use derive_more::Display;
use eyre::{Result, WrapErr};
use futures::Stream;
use tracing::warn;
use turmoil06::net::{TcpListener, TcpStream};

pub(super) use turmoil06::net::tcp::{OwnedReadHalf, OwnedWriteHalf};

const PORT: u16 = 0xE1F0;

#[derive(Clone, Display)]
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

pub(super) async fn connect(host: &str) -> Result<Socket> {
    prepare_stream(TcpStream::connect((host, PORT)).await?)
}

pub(super) async fn listen(host: &str) -> Result<impl Stream<Item = Socket> + 'static> {
    let listener = TcpListener::bind((host, PORT)).await?;

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
