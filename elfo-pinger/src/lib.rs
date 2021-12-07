#![warn(rust_2018_idioms, unreachable_pub)]

use elfo_core::{ActorGroup, Schema, Topology};

mod actor;
mod config;

pub fn new(topology: &Topology) -> Schema {
    let topology = topology.clone();
    ActorGroup::new()
        .config::<config::Config>()
        .exec(move |ctx| actor::exec(ctx, topology.clone()))
}
