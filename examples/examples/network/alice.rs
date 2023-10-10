use std::time::Duration;

use elfo::{prelude::*, time::Interval, topology::Outcome};
use tracing::{info, warn};

use crate::protocol::{AskName, Hello};

fn producer() -> Blueprint {
    #[message]
    struct TimerTick;

    ActorGroup::new().exec(|mut ctx| async move {
        ctx.attach(Interval::new(TimerTick))
            .start(Duration::from_secs(1));

        let mut i = 0;

        while let Some(envelope) = ctx.recv().await {
            msg!(match envelope {
                TimerTick => {
                    match ctx.request(AskName(i)).resolve().await {
                        Ok(name) => info!("received name: {}", name),
                        Err(err) => warn!("cannot ask name: {}", err),
                    }

                    match ctx.send(Hello(i)).await {
                        Ok(_) => i += 1,
                        Err(err) => warn!("cannot say hello: {}", err),
                    }
                }
                Hello(i) => {
                    info!("received Hello({})", i);
                }
            });
        }
    })
}

pub(crate) fn topology() -> elfo::Topology {
    elfo::_priv::node::set_node_no(1);

    let topology = elfo::Topology::empty();
    let logger = elfo::batteries::logger::init();
    let telemeter = elfo::batteries::telemeter::init();

    // System groups.
    let loggers = topology.local("system.loggers");
    let telemeters = topology.local("system.telemeters");
    let dumpers = topology.local("system.dumpers");
    let configurers = topology.local("system.configurers").entrypoint();
    let network = topology.local("system.network");

    // Local user groups.
    let producers = topology.local("producers");

    // Remote user groups.
    let consumers = topology.remote("consumers");

    producers.route_to(&consumers, |_, _| Outcome::Broadcast);

    loggers.mount(logger);
    telemeters.mount(telemeter);
    dumpers.mount(elfo::batteries::dumper::new());
    network.mount(elfo::batteries::network::new(&topology));
    producers.mount(producer());

    let config_path = "examples/examples/network/alice.toml";
    configurers.mount(elfo::batteries::configurer::from_path(
        &topology,
        config_path,
    ));

    topology
}
