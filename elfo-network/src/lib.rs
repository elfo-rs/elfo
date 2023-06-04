//! TODO

#![warn(rust_2018_idioms, unreachable_pub, missing_docs)]

#[macro_use]
extern crate elfo_utils;

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

use crate::{config::Config, node_map::NodeMap, protocol::HandleConnection};

mod codec;
mod config;
mod connection;
mod discovery;
mod node_map;
mod protocol;
mod socket;

#[derive(PartialEq, Eq, Hash, Clone)]
enum ActorKey {
    Discovery,
    Connection {
        local: GroupNo,
        remote: (NodeNo, GroupNo),
    },
}

impl Display for ActorKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // TODO: resolve `group_no` to name.
        match self {
            ActorKey::Discovery => f.write_str("discovery"),
            ActorKey::Connection { local, remote } => {
                write!(f, "{}:{}:{}", local, remote.0, remote.1)
            }
        }
    }
}

type NetworkContext = Context<Config, ActorKey>;

/// TODO
pub fn new(topology: &Topology) -> Blueprint {
    let node_map = Arc::new(NodeMap::new(topology));

    ActorGroup::new()
        .config::<Config>()
        .router(MapRouter::new(|envelope| {
            msg!(match envelope {
                UpdateConfig => Outcome::Unicast(ActorKey::Discovery),
                msg @ HandleConnection => Outcome::Unicast(ActorKey::Connection {
                    local: msg.local,
                    remote: msg.remote,
                }),
                _ => Outcome::Default,
            })
        }))
        .exec(move |ctx: Context<Config, ActorKey>| {
            let node_map = node_map.clone();

            async move {
                match *ctx.key() {
                    ActorKey::Discovery => discovery::Discovery::new(ctx, node_map).main().await,
                    ActorKey::Connection { local, remote } => {
                        connection::Connection::new(ctx, local, remote).main().await
                    }
                }
            }
        })
}
