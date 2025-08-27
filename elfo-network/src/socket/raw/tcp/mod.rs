use std::net::SocketAddr;

use derive_more::Display;
use eyre::{Result, WrapErr};
use futures::Stream;
use tokio::net::{TcpListener, TcpStream};
use tracing::warn;

pub(super) use tokio::net::tcp::OwnedReadHalf;

use std::{
    io::{IoSlice, Result as IoResult},
    pin::Pin,
    task::{Context, Poll},
};
use tokio::io::AsyncWrite;

#[cfg(target_os = "linux")]
mod metrics;

#[cfg(target_os = "linux")]
use self::metrics::TcpMetrics;

pub(in crate::socket) struct OwnedWriteHalf {
    inner: tokio::net::tcp::OwnedWriteHalf,
    #[cfg(target_os = "linux")]
    metrics: TcpMetrics,
}

impl OwnedWriteHalf {
    pub(crate) fn collect_transport_specific_metrics(&mut self) {
        #[cfg(target_os = "linux")]
        {
            match metrics::collect_tcp_metrics(self.inner.as_ref()) {
                Ok(current_metrics) => {
                    metrics::report_tcp_metrics(&current_metrics, &self.metrics);
                    self.metrics = current_metrics;
                }
                Err(err) => {
                    tracing::warn!(
                        message = "failed to collect TCP metrics",
                        error = %err,
                    );
                }
            }
        }
    }
}

impl AsyncWrite for OwnedWriteHalf {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<IoResult<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<IoResult<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<IoResult<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[IoSlice<'_>],
    ) -> Poll<IoResult<usize>> {
        Pin::new(&mut self.inner).poll_write_vectored(cx, bufs)
    }

    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }
}

#[derive(Clone, Display, PartialEq, Eq)]
#[display("tcp(local={local}, peer={peer})")] // TODO: use `valuable` after tracing#1570
pub(crate) struct SocketInfo {
    local: SocketAddr,
    peer: SocketAddr,
}

impl SocketInfo {
    pub(crate) fn new(local: SocketAddr, peer: SocketAddr) -> Self {
        Self { local, peer }
    }
}

pub(super) struct Socket {
    pub(super) read: OwnedReadHalf,
    pub(super) write: OwnedWriteHalf,
    pub(super) info: SocketInfo,
}

fn prepare_stream(stream: TcpStream) -> Result<Socket> {
    let local = stream.local_addr().wrap_err("cannot get local addr")?;
    let peer = stream.peer_addr().wrap_err("cannot get peer addr")?;
    let info = SocketInfo::new(local, peer);

    // TODO: settings (keepalive, linger, etc.)
    if let Err(err) = stream.set_nodelay(true) {
        warn!(
            message = "cannot turn off Nagle's algorithm",
            reason = %err,
            socket = %info,
        );
    }

    let (read, write) = stream.into_split();
    let write = OwnedWriteHalf {
        inner: write,
        #[cfg(target_os = "linux")]
        metrics: TcpMetrics::default(),
    };
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
