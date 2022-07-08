use std::{net::SocketAddr, sync::Arc};

use eyre::{Result, WrapErr};
use futures::StreamExt;
use tokio::net::{TcpListener, TcpStream};
use tokio_util::codec::{FramedRead, FramedWrite};
use tracing::{info, warn};

use elfo_core as elfo;
use elfo_core::{stream::Stream, Context, Envelope, MoveOwnership};
use elfo_macros::{message, msg_raw as msg};

use crate::{
    actor::Key,
    codec::{Decoder, Encoder},
    config::Config,
    node_map::NodeMap,
};

const MAX_FRAME_SIZE: u32 = 65536; // TODO: make it configurable.

pub(crate) struct Listener {
    node_map: Arc<NodeMap>,
}

#[message(elfo = elfo_core)]
struct TcpConnectionAccepted {
    stream: MoveOwnership<TcpStream>,
    addr: SocketAddr,
}

impl Listener {
    pub(crate) fn new(node_map: Arc<NodeMap>) -> Self {
        Self { node_map }
    }

    pub(crate) async fn main(self, ctx: Context<Config, Key>) -> Result<()> {
        // TODO: timeout.
        let listener = TcpListener::bind(&ctx.config().listener.tcp)
            .await
            .wrap_err("cannot bind TCP listener")?;

        let mut ctx = ctx.with(make_listener_stream(listener));

        info!(
            message = "listening for TCP connections",
            addr = %ctx.config().listener.tcp
        );

        while let Some(envelope) = ctx.recv().await {
            msg!(match envelope {
                msg @ TcpConnectionAccepted => {
                    info!(message = "new TCP connection accepted", remote_addr = %msg.addr);

                    let stream = msg.stream.take().unwrap();
                    // TODO: apply settings (linger, nodelay) here?

                    let (rx, tx) = stream.into_split();
                    let mut rx = FramedRead::new(rx, Decoder::new(MAX_FRAME_SIZE));
                    let tx = FramedWrite::new(tx, Encoder::new(MAX_FRAME_SIZE));

                    // TODO: timeout
                    if let Some(res) = rx.next().await {
                        // TODO: no error.
                        let (envelope, target_addr) = res?;

                        info!(message = "GOT IT!", name = ?envelope.message(), %target_addr);
                    }
                }
            });
        }

        Ok(())
    }
}

fn make_listener_stream(listener: TcpListener) -> Stream<impl futures::Stream<Item = Envelope>> {
    Stream::generate(move |mut y| async move {
        loop {
            match listener.accept().await {
                Ok((stream, addr)) => {
                    let stream = stream.into();
                    y.emit(TcpConnectionAccepted { stream, addr }).await;
                }
                Err(err) => {
                    warn!(message = "cannot accept TCP connection", error = %err);
                }
            }
        }
    })
}
