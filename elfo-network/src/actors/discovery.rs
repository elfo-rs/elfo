use std::{sync::Arc, time::Duration};

use eyre::Result;
use futures::SinkExt;
use tokio::net::TcpStream;
use tokio_util::codec::{FramedRead, FramedWrite};
use tracing::info;

use elfo_core::{message, msg, scope, time::Interval, Addr, Context, Envelope, _priv::MessageKind};

use crate::{
    actors::Key,
    codec::{Decoder, Encoder},
    config::Config,
    node_map::NodeMap,
    protocol::{Greeting, RemoteGroupInfo},
};

const MAX_FRAME_SIZE: u32 = 65536; // TODO: make it configurable.

pub(crate) struct Discovery {
    ctx: Context<Config, Key>,
    node_map: Arc<NodeMap>,
}

#[message(elfo = elfo_core)]
struct DiscoveryTick;

impl Discovery {
    pub(crate) fn new(ctx: Context<Config, Key>, node_map: Arc<NodeMap>) -> Self {
        Self { ctx, node_map }
    }

    pub(crate) async fn main(mut self) -> Result<()> {
        let interval = self.ctx.attach(Interval::new(DiscoveryTick));
        interval.start_after(Duration::from_secs(0), Duration::from_secs(10)); // TODO: configurable.

        while let Some(envelope) = self.ctx.recv().await {
            msg!(match envelope {
                DiscoveryTick => self.discover().await?,
            });
        }

        Ok(())
    }

    async fn discover(&mut self) -> Result<()> {
        // TODO: do it in parallel.
        for addr in &self.ctx.config().discovery.predefined {
            info!(message = "discovering", %addr);

            // TODO: timeout, skip errors.
            let stream = TcpStream::connect(addr).await?;

            let (rx, tx) = stream.into_split();
            let rx = FramedRead::new(rx, Decoder::new(MAX_FRAME_SIZE));
            let mut tx = FramedWrite::new(tx, Encoder::new(MAX_FRAME_SIZE));

            let envelope = Envelope::with_trace_id(
                Greeting {
                    node_no: self.node_map.this.node_no,
                    groups: self
                        .node_map
                        .this
                        .groups
                        .iter()
                        .map(|info| RemoteGroupInfo {
                            group_no: info.group_no,
                            name: info.name.clone(),
                        })
                        .collect(),
                },
                MessageKind::Regular { sender: Addr::NULL },
                scope::trace_id(),
            )
            .upcast();
            tx.send((envelope, Addr::NULL)).await?;
        }

        Ok(())
    }
}
