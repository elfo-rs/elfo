use elfo::prelude::*;
use tracing::{info, warn};

use crate::protocol::{AskName, Hello};

fn consumer() -> Blueprint {
    ActorGroup::new().exec(|mut ctx| async move {
        while let Some(envelope) = ctx.recv().await {
            let sender = envelope.sender();

            msg!(match envelope {
                Hello(i) => {
                    info!("received Hello({})", i);

                    if let Err(err) = ctx.send_to(sender, Hello(i)).await {
                        warn!("cannot say Hello({}) back: {}", i, err);
                    }
                }
                (AskName, token) => {
                    info!("asked for name");
                    ctx.respond(token, "Bob".into());
                }
            });
        }
    })
}

pub(crate) fn topology() -> elfo::Topology {
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
    let consumers = topology.local("consumers");

    loggers.mount(logger);
    telemeters.mount(telemeter);
    dumpers.mount(elfo::batteries::dumper::new());
    network.mount(elfo::batteries::network::new(&topology));
    consumers.mount(consumer());

    let config_path = "examples/network/bob.toml";
    configurers.mount(elfo::batteries::configurer::from_path(
        &topology,
        config_path,
    ));

    topology
}
