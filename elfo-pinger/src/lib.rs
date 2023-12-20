#![warn(rust_2018_idioms, unreachable_pub)]

use std::time::Duration;

use elfo_core::{ActorGroup, Blueprint, RestartParams, RestartPolicy, Topology};

mod actor;
mod config;

pub fn new(topology: &Topology) -> Blueprint {
    let topology = topology.clone();
    ActorGroup::new()
        .config::<config::Config>()
        .restart_policy(RestartPolicy::on_failure(RestartParams::new(
            Duration::from_secs(5),
            Duration::from_secs(30),
        )))
        .stop_order(100)
        .exec(move |ctx| actor::exec(ctx, topology.clone()))
}
