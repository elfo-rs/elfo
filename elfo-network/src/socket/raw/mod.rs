use std::{
    io::{IoSlice, Result as IoResult},
    pin::Pin,
    task::{Context, Poll},
};

use derive_more::Display;
use eyre::Result;
use futures::{stream::BoxStream, StreamExt};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

use crate::config::Transport;

mod tcp;
#[cfg(feature = "turmoil06")]
mod turmoil;
mod uds;

macro_rules! delegate_call {
    ($self:ident.$method:ident($($args:ident),+)) => {
        match $self.get_mut() {
            Self::Tcp(v) => Pin::new(v).$method($($args),+),
            #[cfg(unix)]
            Self::Uds(v) => Pin::new(v).$method($($args),+),
            #[cfg(feature = "turmoil06")]
            Self::Turmoil06(v) => Pin::new(v).$method($($args),+),
        }
    }
}

#[derive(Clone, Display, PartialEq, Eq)]
pub(crate) enum SocketInfo {
    Tcp(tcp::SocketInfo),
    #[cfg(unix)]
    Uds(uds::SocketInfo),
    #[cfg(feature = "turmoil06")]
    Turmoil06(turmoil::SocketInfo),
}

impl SocketInfo {
    #[cfg(test)]
    pub(crate) fn tcp(local: std::net::SocketAddr, peer: std::net::SocketAddr) -> Self {
        Self::Tcp(tcp::SocketInfo::new(local, peer))
    }
}

pub(super) enum OwnedReadHalf {
    Tcp(tcp::OwnedReadHalf),
    #[cfg(unix)]
    Uds(uds::OwnedReadHalf),
    #[cfg(feature = "turmoil06")]
    Turmoil06(turmoil::OwnedReadHalf),
}

impl AsyncRead for OwnedReadHalf {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<IoResult<()>> {
        delegate_call!(self.poll_read(cx, buf))
    }
}

pub(super) enum OwnedWriteHalf {
    Tcp(tcp::OwnedWriteHalf),
    #[cfg(unix)]
    Uds(uds::OwnedWriteHalf),
    #[cfg(feature = "turmoil06")]
    Turmoil06(turmoil::OwnedWriteHalf),
}

impl AsyncWrite for OwnedWriteHalf {
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<IoResult<usize>> {
        delegate_call!(self.poll_write(cx, buf))
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<IoResult<()>> {
        delegate_call!(self.poll_flush(cx))
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<IoResult<()>> {
        delegate_call!(self.poll_shutdown(cx))
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[IoSlice<'_>],
    ) -> Poll<IoResult<usize>> {
        delegate_call!(self.poll_write_vectored(cx, bufs))
    }

    fn is_write_vectored(&self) -> bool {
        match self {
            Self::Tcp(v) => v.is_write_vectored(),
            #[cfg(unix)]
            Self::Uds(v) => v.is_write_vectored(),
            #[cfg(feature = "turmoil06")]
            Self::Turmoil06(v) => v.is_write_vectored(),
        }
    }
}

pub(super) struct Socket {
    pub(super) read: OwnedReadHalf,
    pub(super) write: OwnedWriteHalf,
    pub(super) info: SocketInfo,
}

impl From<tcp::Socket> for Socket {
    fn from(socket: tcp::Socket) -> Self {
        Self {
            read: OwnedReadHalf::Tcp(socket.read),
            write: OwnedWriteHalf::Tcp(socket.write),
            info: SocketInfo::Tcp(socket.info),
        }
    }
}

#[cfg(unix)]
impl From<uds::Socket> for Socket {
    fn from(socket: uds::Socket) -> Self {
        Self {
            read: OwnedReadHalf::Uds(socket.read),
            write: OwnedWriteHalf::Uds(socket.write),
            info: SocketInfo::Uds(socket.info),
        }
    }
}

#[cfg(feature = "turmoil06")]
impl From<turmoil::Socket> for Socket {
    fn from(socket: turmoil::Socket) -> Self {
        Self {
            read: OwnedReadHalf::Turmoil06(socket.read),
            write: OwnedWriteHalf::Turmoil06(socket.write),
            info: SocketInfo::Turmoil06(socket.info),
        }
    }
}

pub(super) async fn connect(addr: &Transport) -> Result<Socket> {
    match addr {
        Transport::Tcp(addr) => tcp::connect(addr).await.map(Into::into),
        #[cfg(unix)]
        Transport::Uds(addr) => uds::connect(addr).await.map(Into::into),
        #[cfg(feature = "turmoil06")]
        Transport::Turmoil06(addr) => turmoil::connect(addr).await.map(Into::into),
    }
}

pub(super) async fn listen(addr: &Transport) -> Result<BoxStream<'static, Socket>> {
    Ok(match addr {
        Transport::Tcp(addr) => Box::pin(tcp::listen(addr).await?.map(Into::into)),
        #[cfg(unix)]
        Transport::Uds(addr) => Box::pin(uds::listen(addr)?.map(Into::into)),
        #[cfg(feature = "turmoil06")]
        Transport::Turmoil06(addr) => Box::pin(turmoil::listen(addr).await?.map(Into::into)),
    })
}
