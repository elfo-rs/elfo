//! Periodically pings all actors in the topology to check if they are alive.
//! [Configuration].
//!
//! [Configuration]: config::Config

use std::time::Duration;

use elfo_core::{ActorGroup, Blueprint, RestartParams, RestartPolicy, Topology};

pub mod config;

mod actor;

/// Creates a blueprint.
///
/// # Example
/// ```
/// # use elfo_core as elfo;
/// let topology = elfo::Topology::empty();
/// let pingers = topology.local("pingers");
///
/// // Usually, it's `elfo::batteries::pinger::fixture`.
/// pingers.mount(elfo_pinger::new(&topology));
/// ```
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
