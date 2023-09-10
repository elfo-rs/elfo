//! TODO

#![warn(rust_2018_idioms, unreachable_pub, missing_docs)]

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
    node::NodeNo,
    routers::{MapRouter, Outcome},
    ActorGroup, Blueprint, Context, GroupNo, RestartPolicy, Topology,
};

use crate::{config::Config, protocol::HandleDataConnection};

mod codec;
mod config;
mod connection;
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
    Worker {
        local: (GroupNo, String),
        remote: (NodeNo, GroupNo, String),
    },
}

impl Display for ActorKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // TODO: resolve `group_no` to name.
        match self {
            ActorKey::Discovery => f.write_str("discovery"),
            ActorKey::Worker { local, remote } => {
                write!(f, "{}:{}:{}", local.1, remote.0, remote.2)
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
        // The restart policy is overrided by the discovery actor.
        .restart_policy(RestartPolicy::never())
        .router(MapRouter::new(|envelope| {
            msg!(match envelope {
                // TODO: send to all connections.
                UpdateConfig => Outcome::Unicast(ActorKey::Discovery),
                msg @ HandleDataConnection => Outcome::Unicast(ActorKey::Worker {
                    local: msg.local.clone(),
                    remote: msg.remote.clone(),
                }),
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
