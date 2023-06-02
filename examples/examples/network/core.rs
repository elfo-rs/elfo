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
    let keepers = topology.local("keepers");
    let robots = topology.local("robots");

    // Remote user groups.
    // let collectors = topology.remote("collectors");
    // let management = topology.remote("management");

    // management.route_all_to(robots);
    // collectors.route_all_to(keepers);

    keepers.mount(noop());
    robots.mount(noop());
    loggers.mount(logger);
    telemeters.mount(telemeter);
    dumpers.mount(elfo::batteries::dumper::new());
    pingers.mount(elfo::batteries::pinger::new(&topology));
    network.mount(elfo::batteries::network::new(&topology));

    let config_path = "examples/examples/network/core.toml";
    configurers.mount(elfo::batteries::configurer::from_path(
        &topology,
        config_path,
    ));

    topology
}

// 1. send_to(group_addr)
//      group_addr => (node_no, group_no)
// 2. send_to(actor_addr)
//      actor_addr => (node_no, group_no)
// 3. send()
//   Broadcast => for all node_no, group_no in node_no
//   Nodes(node) => ..

// lsn: system.network.listener
// dsn: system.network.discovery
// out: system.network.tx:12:collectors (tcp/uds to 12.management)
//  in: system.network.rx:16:collectors
