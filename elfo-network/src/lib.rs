//! Implements the network layer of the distributed elfo system.
//! [Configuration].
//!
//! [Configuration]: config::Config

#[macro_use]
extern crate static_assertions;
#[macro_use]
extern crate elfo_utils;

use std::{
    fmt::{self, Display},
    hash::Hash,
};

use elfo_core::{
    messages::UpdateConfig,
    msg,
    routers::{MapRouter, Outcome},
    ActorGroup, Blueprint, Context, Topology,
};

use crate::{
    config::Config,
    protocol::{ConnectionFailed, GroupInfo, HandleConnection},
};

pub mod config;

mod codec;
mod discovery;
mod frame;
mod node_map;
mod protocol;
mod rtt;
mod socket;
mod worker;

#[derive(PartialEq, Eq, Hash, Clone)]
enum ActorKey {
    Discovery,
    Worker { local: GroupInfo, remote: GroupInfo },
}

impl Display for ActorKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ActorKey::Discovery => f.write_str("discovery"),
            ActorKey::Worker { local, remote } => {
                write!(
                    f,
                    "{}:{}:{}",
                    local.group_name, remote.node_no, remote.group_name
                )
            }
        }
    }
}

type NetworkContext = Context<Config, ActorKey>;

/// TODO
pub fn new(topology: &Topology) -> Blueprint {
    let topology = topology.clone();

    ActorGroup::new()
        .config::<Config>()
        .stop_order(100)
        .router(MapRouter::new(|envelope| {
            msg!(match envelope {
                // TODO: send to all connections.
                UpdateConfig => Outcome::Unicast(ActorKey::Discovery),
                msg @ HandleConnection => Outcome::Unicast(ActorKey::Worker {
                    local: msg.local.clone(),
                    remote: msg.remote.clone(),
                }),
                ConnectionFailed => Outcome::Unicast(ActorKey::Discovery),
                _ => Outcome::Default,
            })
        }))
        .exec(move |ctx: Context<Config, ActorKey>| {
            let topology = topology.clone();
            async move {
                match ctx.key().clone() {
                    ActorKey::Discovery => discovery::Discovery::new(ctx, topology).main().await,
                    ActorKey::Worker { local, remote } => {
                        worker::Worker::new(ctx, local, remote, topology)
                            .main()
                            .await
                    }
                }
            }
        })
}
