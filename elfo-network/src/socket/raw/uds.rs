#![cfg(unix)]

use std::{
    fmt,
    path::{Path, PathBuf},
};

use eyre::Result;
use futures::Stream;
use tokio::net::{unix::pid_t as Pid, UnixListener, UnixStream};
use tracing::warn;

pub(super) use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};

#[derive(Clone)]
pub(crate) struct SocketInfo {
    path: PathBuf,
    peer_pid: Option<Pid>,
}

// TODO: use `valuable` after tracing#1570
impl fmt::Display for SocketInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        let path = self.path.display();
        if let Some(peer_pid) = self.peer_pid {
            write!(f, "uds(path={path}, peer_pid={peer_pid})")
        } else {
            write!(f, "uds(path={path})")
        }
    }
}

pub(super) struct Socket {
    pub(super) read: OwnedReadHalf,
    pub(super) write: OwnedWriteHalf,
    pub(super) info: SocketInfo,
}

fn prepare_stream(stream: UnixStream, path: &Path) -> Socket {
    let info = SocketInfo {
        path: path.to_owned(),
        peer_pid: stream.peer_cred().ok().and_then(|cred| cred.pid()),
    };
    let (read, write) = stream.into_split();
    Socket { read, write, info }
}

pub(super) async fn connect(addr: &Path) -> Result<Socket> {
    Ok(prepare_stream(UnixStream::connect(&addr).await?, addr))
}

pub(super) fn listen(addr: &Path) -> Result<impl Stream<Item = Socket> + 'static> {
    let listener = UnixListener::bind(addr)?;

    let accept = move |(listener, addr): (UnixListener, PathBuf)| async move {
        loop {
            match listener.accept().await {
                Ok((stream, _)) => {
                    let socket = prepare_stream(stream, &addr);
                    return Some((socket, (listener, addr)));
                }
                Err(err) => {
                    warn!(
                        message = "cannot accept UDS connection",
                        error = %err,
                        path = %addr.display(),
                    );

                    // Continue listening.
                }
            }
        }
    };

    Ok(futures::stream::unfold((listener, addr.to_owned()), accept))
}
