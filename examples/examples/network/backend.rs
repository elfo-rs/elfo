use elfo::prelude::*;

fn noop() -> Blueprint {
    ActorGroup::new().exec(|_ctx| async move {})
}

pub(crate) fn topology() -> elfo::Topology {
    let topology = elfo::Topology::empty();
    let logger = elfo::batteries::logger::init();
    let telemeter = elfo::batteries::telemeter::init();

    // System groups.
    let loggers = topology.local("system.loggers");
    let telemeters = topology.local("system.telemeters");
    let dumpers = topology.local("system.dumpers");
    let pingers = topology.local("system.pingers");
    let configurers = topology.local("system.configurers").entrypoint();
    let network = topology.local("system.network");

    // Local user groups.
    let collectors = topology.local("collectors");
    let management = topology.local("management");

    // Remote user groups.
    // let keepers = topology.remote("keepers");
    let robots = topology.remote("robots");

    // management.route_to(&robots, |e, loc| {
    // msg!(match e {
    // SubscribeToActorStatuses => Outcome::Broadcast,
    // Start { node_no, .. } | Stop { node_no, .. } => Outcome::Node(node_no),
    //_ => Outcome::Discard,
    //});

    // collectors.route_to(&keepers, |e, loc| {
    // msg!(match e {
    // SubscribeToStream { stream, .. } => Outcome::Broadcast,
    //_ => Outcome::Discard,
    //});

    // keepers.mount(noop());
    // robots.mount(noop());
    loggers.mount(logger);
    telemeters.mount(telemeter);
    dumpers.mount(elfo::batteries::dumper::new());
    pingers.mount(elfo::batteries::pinger::new(&topology));
    network.mount(elfo::batteries::network::new(&topology));

    let config_path = "examples/examples/network/backend.toml";
    configurers.mount(elfo::batteries::configurer::from_path(
        &topology,
        config_path,
    ));

    topology
}
