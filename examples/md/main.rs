use elfo::prelude::*;

mod md;
mod protocol;
mod strategy;

fn topology() -> elfo::Topology {
    let topology = elfo::Topology::empty();
    let logger = elfo::batteries::logger::init();

    // Local groups

    let strategies = topology.local("strategies");
    let md = topology.local("md");

    let loggers = topology.local("system.loggers");
    let configurers = topology.local("system.configurers").entrypoint();

    // Inter-group connections

    strategies.route_to(&md, |envelope| {
        msg!(match envelope {
            protocol::SubscribeToMd => true,
            _ => false,
        })
    });

    // Mount

    strategies.mount(strategy::new());
    md.mount(md::new());

    let config_path = "examples/md/config.toml";
    configurers.mount(elfo::batteries::configurer::from_path(
        &topology,
        config_path,
    ));
    loggers.mount(logger);

    topology
}

#[tokio::main]
async fn main() {
    elfo::init::start(topology()).await;
}

// Not required for demo, but reasonable in real systems.
#[global_allocator]
static ALLOCATOR: mimalloc::MiMalloc = mimalloc::MiMalloc;
