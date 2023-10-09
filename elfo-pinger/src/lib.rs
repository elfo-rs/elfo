#![warn(rust_2018_idioms, unreachable_pub)]

use elfo_core::{ActorGroup, Blueprint, Topology};

mod actor;
mod config;

pub fn new(topology: &Topology) -> Blueprint {
    let topology = topology.clone();
    ActorGroup::new()
        .config::<config::Config>()
        .stop_order(100)
        .exec(move |ctx| actor::exec(ctx, topology.clone()))
}
