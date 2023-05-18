use std::{net::SocketAddr, sync::Arc};

use eyre::{Result, WrapErr};
use futures::StreamExt;
use tokio::net::{TcpListener, TcpStream};
use tokio_util::codec::{FramedRead, FramedWrite};
use tracing::{info, warn};

use elfo_core::{
    message, msg, scope, stream::Stream, tracing::TraceId, Context, MoveOwnership, UnattachedSource,
};

use crate::{
    actors::Key,
    codec::{Decoder, Encoder},
    config::Config,
    node_map::NodeMap,
};

const MAX_FRAME_SIZE: u32 = 65536; // TODO: make it configurable.

pub(crate) struct Listener {
    ctx: Context<Config, Key>,
    node_map: Arc<NodeMap>,
}

#[message(elfo = elfo_core)]
struct TcpConnectionAccepted {
    stream: MoveOwnership<TcpStream>,
    addr: SocketAddr,
}

impl Listener {
    pub(crate) fn new(ctx: Context<Config, Key>, node_map: Arc<NodeMap>) -> Self {
        Self { ctx, node_map }
    }

    pub(crate) async fn main(mut self) -> Result<()> {
        // TODO: timeout.
        let listener = TcpListener::bind(&self.ctx.config().listener.tcp)
            .await
            .wrap_err("cannot bind TCP listener")?;

        info!(
            message = "listening for TCP connections",
            addr = %self.ctx.config().listener.tcp
        );
        self.ctx.attach(make_listener_stream(listener));

        while let Some(envelope) = self.ctx.recv().await {
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

fn make_listener_stream(listener: TcpListener) -> UnattachedSource<Stream> {
    Stream::generate(move |mut y| async move {
        loop {
            scope::set_trace_id(TraceId::generate());

            match listener.accept().await {
                Ok((stream, addr)) => {
                    let stream = stream.into();
                    y.emit(TcpConnectionAccepted { stream, addr }).await;
                }
                Err(err) => {
                    warn!(message = "cannot accept TCP connection", error = %err);
                    // TODO: restart?
                }
            }
        }
    })
}
