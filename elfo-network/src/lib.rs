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
    group::RestartPolicy,
    messages::UpdateConfig,
    msg,
    node::NodeNo,
    routers::{MapRouter, Outcome},
    ActorGroup, Blueprint, Context, GroupNo, Topology,
};

use crate::{config::Config, protocol::HandleConnection};

mod codec;
mod config;
mod connection;
mod discovery;
mod flow_control;
mod node_map;
mod protocol;
mod socket;

#[derive(PartialEq, Eq, Hash, Clone)]
enum ActorKey {
    Discovery,
    Connection {
        local: (GroupNo, String),
        remote: (NodeNo, GroupNo, String),
    },
}

impl Display for ActorKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // TODO: resolve `group_no` to name.
        match self {
            ActorKey::Discovery => f.write_str("discovery"),
            ActorKey::Connection { local, remote } => {
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
        // TODO: actually, we want to restart, but only the discrovery actor.
        .restart_policy(RestartPolicy::never())
        .router(MapRouter::new(|envelope| {
            msg!(match envelope {
                UpdateConfig => Outcome::Unicast(ActorKey::Discovery),
                msg @ HandleConnection => Outcome::Unicast(ActorKey::Connection {
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
                    ActorKey::Connection { local, remote } => {
                        connection::Connection::new(ctx, local, remote, topology)
                            .main()
                            .await
                    }
                }
            }
        })
}
