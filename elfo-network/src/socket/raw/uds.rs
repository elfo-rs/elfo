#![cfg(unix)]

use std::{
    fmt,
    io::Result as IoResult,
    path::{Path, PathBuf},
};

use eyre::Result;
use futures::Stream;
use tokio::net::{unix::pid_t as Pid, UnixListener, UnixStream};
use tracing::{debug, warn};

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
    let listener = Listener::bind(addr)?;

    let accept = move |(listener, addr): (Listener, PathBuf)| async move {
        loop {
            match listener.accept().await {
                Ok(stream) => {
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

/// A wrapper around `UnixListener` which unlinks the socket file during binding
/// and destruction. It requires because the kernel doesn't do it automatically.
///
/// Deletion before binding is controversial because it stops the previous owner
/// of the socket to accept new connections, that can happen in the case of
/// misconfiguration. However, it allows to recover after a crash (or SIGKILL)
/// of the node where destructors aren't called.
struct Listener {
    path: PathBuf,
    inner: UnixListener,
}

impl Listener {
    fn bind(path: &Path) -> IoResult<Self> {
        remove_file_with_log(path);
        let path = path.to_owned();
        UnixListener::bind(&path).map(|inner| Listener { path, inner })
    }

    async fn accept(&self) -> IoResult<UnixStream> {
        self.inner.accept().await.map(|(socket, _)| socket)
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        remove_file_with_log(&self.path);
    }
}

fn remove_file_with_log(path: &Path) {
    match std::fs::remove_file(path) {
        Ok(()) => debug!(message = "removed UDS socket", path = %path.display()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => warn!(
            message = "cannot remove UDS socket",
            error = %err,
            path = %path.display(),
        ),
    }
}
