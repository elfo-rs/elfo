use std::{
    fmt::{self, Display},
    hash::Hash,
    sync::Arc,
};

use elfo_core::{
    messages::UpdateConfig,
    msg,
    node::NodeNo,
    routers::{MapRouter, Outcome},
    ActorGroup, Blueprint, Context, GroupNo, Topology,
};

use self::{discovery::Discovery, receiver::Receiver, transmitter::Transmitter};
use crate::{
    config::Config,
    node_map::NodeMap,
    protocol::{StartReceiver, StartTransmitter},
};

mod discovery;
mod receiver;
mod transmitter;

#[derive(PartialEq, Eq, Hash, Clone)]
pub(crate) enum Key {
    Discovery,
    Tx(NodeNo, GroupNo),
    Rx(NodeNo, GroupNo),
}

impl Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // TODO: resolve `group_no` to name.
        match self {
            Key::Discovery => f.write_str("discovery"),
            Key::Tx(node_no, remote_group_no) => write!(f, "tx:{}:{}", node_no, remote_group_no),
            Key::Rx(node_no, local_group_no) => write!(f, "rx:{}:{}", node_no, local_group_no),
        }
    }
}

/// TODO
pub fn new(topology: &Topology) -> Blueprint {
    let node_map = Arc::new(NodeMap::new(topology));

    ActorGroup::new()
        .config::<Config>()
        .router(MapRouter::new(|envelope| {
            msg!(match envelope {
                UpdateConfig => Outcome::Unicast(Key::Discovery),
                msg @ StartTransmitter =>
                    Outcome::Unicast(Key::Tx(msg.node_no, msg.remote_group_no)),
                msg @ StartReceiver => Outcome::Unicast(Key::Rx(msg.node_no, msg.local_group_no)),
                _ => Outcome::Default,
            })
        }))
        .exec(move |ctx: Context<Config, Key>| {
            let node_map = node_map.clone();

            async move {
                match *ctx.key() {
                    Key::Discovery => Discovery::new(ctx, node_map).main().await,
                    Key::Tx(node_no, remote_group_no) => {
                        Transmitter::new(ctx, node_no, remote_group_no).main().await
                    }
                    Key::Rx(node_no, local_group_no) => {
                        Receiver::new(ctx, node_no, local_group_no).main().await
                    }
                }
            }
        })
}
