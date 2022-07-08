use std::{net::SocketAddr, sync::Arc, time::Duration};

use eyre::{Result, WrapErr};
use futures::{SinkExt, StreamExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_util::codec::{FramedRead, FramedWrite};
use tracing::{info, warn};

use elfo_core as elfo;
use elfo_core::{
    scope, stream::Stream, time::Interval, Addr, Context, Envelope, MoveOwnership,
    _priv::MessageKind,
};
use elfo_macros::{message, msg_raw as msg};

use crate::{
    actor::Key,
    codec::{Decoder, Encoder},
    config::Config,
    node_map::NodeMap,
    protocol::{Greeting, RemoteGroupInfo},
};

const MAX_FRAME_SIZE: u32 = 65536; // TODO: make it configurable.

pub(crate) struct Discovery {
    node_map: Arc<NodeMap>,
}

#[message(elfo = elfo_core)]
struct DiscoveryTick;

impl Discovery {
    pub(crate) fn new(node_map: Arc<NodeMap>) -> Self {
        Self { node_map }
    }

    pub(crate) async fn main(self, ctx: Context<Config, Key>) -> Result<()> {
        let interval = Interval::new(|| DiscoveryTick).after(Duration::from_secs(0));
        interval.set_period(Duration::from_secs(10)); // TODO: configurable.

        let mut ctx = ctx.with(&interval);

        while let Some(envelope) = ctx.recv().await {
            msg!(match envelope {
                DiscoveryTick => {
                    for addr in &ctx.config().discovery.predefined {
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
                }
            })
        }

        Ok(())
    }
}
