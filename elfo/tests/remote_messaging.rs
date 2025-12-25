#![allow(missing_docs)]
#![cfg(feature = "network")]
#![cfg(feature = "turmoil07")]

use std::{sync::Arc, time::Duration};

use serde::Deserialize;
use tokio::sync::{mpsc, Notify};
use toml::toml;
use tracing::{debug, info, warn};

use elfo::{
    messages::UpdateConfig,
    prelude::*,
    routers::{MapRouter, Outcome, Singleton},
    stream::Stream,
    time::Interval,
    topology, Topology,
};

mod common;

#[message]
struct SimpleMessage(u64);

#[message]
struct ProducerTick;

fn simple_producer() -> Blueprint {
    ActorGroup::new().exec(move |mut ctx| async move {
        ctx.attach(Interval::new(ProducerTick))
            .start(Duration::from_secs(1));

        let mut counter = 0;
        while let Some(envelope) = ctx.recv().await {
            msg!(match envelope {
                ProducerTick => {
                    let res = ctx.send(SimpleMessage(counter)).await;
                    info!("sent message #{counter} => {res:?}");
                    counter += 1;
                }
            })
        }
    })
}

fn simple_consumer() -> Blueprint {
    ActorGroup::new().exec(move |mut ctx| async move {
        while let Some(envelope) = ctx.recv().await {
            msg!(match envelope {
                SimpleMessage(no) => {
                    info!("received message #{no}");
                }
            })
        }
    })
}

fn turmoil_sim() -> turmoil::Sim<'static> {
    turmoil::Builder::new()
        // TODO: We don't actually use I/O, but otherwise the test panics with:
        //  "there is no signal driver running"
        // Need to detect availability of the signal driver in the `Signal` source.
        .enable_tokio_io()
        .tick_duration(Duration::from_millis(100))
        .build()
}

#[test]
fn simple() {
    common::setup_logger();

    fn consumer(notify: Arc<Notify>) -> Blueprint {
        ActorGroup::new().exec(move |mut ctx| {
            let notify = notify.clone();
            async move {
                let mut nos = vec![];

                while let Some(envelope) = ctx.recv().await {
                    msg!(match envelope {
                        SimpleMessage(no) => {
                            info!("received message #{no}");
                            nos.push(no);

                            if no == 2 {
                                turmoil::partition("server", "client");
                                tokio::time::sleep(Duration::from_secs(1)).await;
                                turmoil::repair("server", "client");
                            } else if no == 8 {
                                break;
                            }
                        }
                    })
                }

                assert!(nos == [0, 1, 2, 6, 7, 8] || nos == [0, 1, 2, 5, 6, 7, 8]);

                // Terminate the test.
                // TODO: expose `system.init` and use `send(TerminateSystem)` instead.
                notify.notify_one();
            }
        })
    }

    let mut sim = turmoil_sim();

    sim.host("server", || async {
        let topology = Topology::empty();
        let configurers = topology.local("system.configurers").entrypoint();
        let network = topology.local("system.network");
        let producers = topology.local("producers");
        let consumers = topology.remote("consumers");

        producers.route_to(&consumers, |_, _| topology::Outcome::Broadcast);

        network.mount(elfo::batteries::network::new(&topology));
        configurers.mount(elfo::batteries::configurer::fixture(
            &topology,
            toml! {
                [system.network]
                listen = ["turmoil07://0.0.0.0"]
                ping_interval = "1s"
                idle_timeout = "1s"
            },
        ));
        producers.mount(simple_producer());

        Ok(elfo::init::try_start(topology).await?)
    });

    sim.client("client", async {
        let topology = Topology::empty();
        let configurers = topology.local("system.configurers").entrypoint();
        let network = topology.local("system.network");
        let consumers = topology.local("consumers");

        network.mount(elfo::batteries::network::new(&topology));
        configurers.mount(elfo::batteries::configurer::fixture(
            &topology,
            toml! {
                [system.network]
                discovery.predefined = ["turmoil07://server"]
                discovery.attempt_interval = "1s"
                ping_interval = "1s"
                idle_timeout = "1s"
            },
        ));

        let notify = Arc::new(Notify::new());
        consumers.mount(consumer(notify.clone()));

        Ok(elfo::_priv::do_start(topology, false, |_, _| async move {
            notify.notified().await;
        })
        .await?)
    });

    sim.run().unwrap();
}

#[test]
fn remote_actor_terminates_without_waiting_for_response() {
    common::setup_logger();

    #[message(ret = SimpleRequestResponse)]
    struct SimpleRequest;

    #[message]
    struct SimpleRequestResponse;

    #[message]
    struct SimpleRequestReceived;

    #[message]
    struct Ping;

    fn client(notify: Arc<Notify>) -> Blueprint {
        ActorGroup::new()
            .router(MapRouter::new(|e| {
                msg!(match e {
                    UpdateConfig => Outcome::Unicast(Singleton),
                    _ => Outcome::Discard,
                })
            }))
            .exec(move |mut ctx| {
                let notify = notify.clone();
                async move {
                    ctx.attach(Stream::once({
                        let pruned = ctx.pruned();
                        async move {
                            loop {
                                match pruned.request(SimpleRequest).resolve().await {
                                    Ok(response) => break response,
                                    Err(err) => {
                                        debug!(?err, "failed to send request");
                                        tokio::time::sleep(Duration::from_millis(100)).await;
                                    }
                                }
                            }
                        }
                    }));
                    while let Some(envelope) = ctx.recv().await {
                        let _sender = envelope.sender();
                        msg!(match envelope {
                            SimpleRequestReceived => {
                                info!("request received by remote server, terminating...");
                                break;
                            }
                            SimpleRequestResponse => unreachable!(),
                            msg => debug!(?msg, "unknown message"),
                        })
                    }

                    // Terminate the test.
                    // TODO: expose `system.init` and use `send(TerminateSystem)` instead.
                    notify.notify_one();
                }
            })
    }

    fn server(notify: Arc<Notify>) -> Blueprint {
        ActorGroup::new()
            .router(MapRouter::new(|e| {
                msg!(match e {
                    SimpleRequest => Outcome::Unicast(Singleton),
                    _ => Outcome::Discard,
                })
            }))
            .exec(move |mut ctx| {
                let notify = notify.clone();
                async move {
                    let mut producer = None;
                    let mut token = None;
                    while let Some(envelope) = ctx.recv().await {
                        let sender = envelope.sender();
                        msg!(match envelope {
                            (SimpleRequest, tok) => {
                                info!("received received from remote client");
                                token = Some(tok);
                                producer = Some(sender);
                                ctx.unbounded_send_to(sender, SimpleRequestReceived)
                                    .unwrap();
                                break;
                            }
                            msg => tracing::error!(?msg, "unknown message"),
                        });
                    }

                    while ctx.send_to(producer.unwrap(), Ping).await.is_ok() {}
                    ctx.respond(token.unwrap(), SimpleRequestResponse);

                    // Terminate the test.
                    // TODO: expose `system.init` and use `send(TerminateSystem)` instead.
                    notify.notify_one();
                }
            })
    }

    let mut sim = turmoil::Builder::new()
        // TODO: We don't actually use I/O, but otherwise the test panics with:
        //  "there is no signal driver running"
        // Need to detect availability of the signal driver in the `Signal` source.
        .enable_tokio_io()
        .tick_duration(Duration::from_millis(100))
        .build();

    sim.client("server", async {
        let topology = Topology::empty();
        let configurers = topology.local("system.configurers").entrypoint();
        let network = topology.local("system.network");
        let consumers = topology.local("consumers");

        network.mount(elfo::batteries::network::new(&topology));
        configurers.mount(elfo::batteries::configurer::fixture(
            &topology,
            toml! {
                [system.network]
                listen = ["turmoil07://0.0.0.0"]
                ping_interval = "1s"
                idle_timeout = "1s"
            },
        ));

        let notify = Arc::new(Notify::new());
        consumers.mount(server(notify.clone()));

        Ok(elfo::_priv::do_start(topology, false, |_, _| async move {
            notify.notified().await;
        })
        .await?)
    });

    sim.client("client", async {
        let topology = Topology::empty();
        let configurers = topology.local("system.configurers").entrypoint();
        let network = topology.local("system.network");
        let producers = topology.local("producers");
        let consumers = topology.remote("consumers");

        producers.route_to(&consumers, |_, _| topology::Outcome::Broadcast);

        network.mount(elfo::batteries::network::new(&topology));
        configurers.mount(elfo::batteries::configurer::fixture(
            &topology,
            toml! {
                [system.network]
                discovery.predefined = ["turmoil07://server"]
                ping_interval = "1s"
                idle_timeout = "1s"
            },
        ));

        let notify = Arc::new(Notify::new());
        producers.mount(client(notify.clone()));

        Ok(elfo::_priv::do_start(topology, false, |_, _| async move {
            notify.notified().await;
        })
        .await?)
    });

    sim.run().unwrap();
}

#[test]
fn discovery_predefined_changed() {
    common::setup_logger();

    fn client_config(server_port: u16) -> toml::Value {
        let server_addr = format!("turmoil07://server:{server_port}");
        toml! {
            [system.network]
            discovery.predefined = [server_addr]
            discovery.attempt_interval = "1s"
            ping_interval = "1s"
            idle_timeout = "1s"
        }
        .into()
    }

    let mut sim = turmoil_sim();

    sim.host("server", || async {
        let topology = Topology::empty();
        let configurers = topology.local("system.configurers").entrypoint();
        let network = topology.local("system.network");
        let producers = topology.local("producers");
        let consumers = topology.remote("consumers");

        producers.route_to(&consumers, |_, _| topology::Outcome::Broadcast);

        network.mount(elfo::batteries::network::new(&topology));
        configurers.mount(elfo::batteries::configurer::fixture(
            &topology,
            toml! {
                [system.network]
                listen = [
                    "turmoil07://0.0.0.0:10000",
                    "turmoil07://0.0.0.0:11111",
                ]
                ping_interval = "1s"
                idle_timeout = "1s"
            },
        ));
        producers.mount(simple_producer());

        Ok(elfo::init::try_start(topology).await?)
    });

    let (switch_tx, mut switch_rx) = mpsc::unbounded_channel();

    sim.client("client", async {
        let topology = Topology::empty();
        let configurers = topology.local("system.configurers").entrypoint();
        let network = topology.local("system.network");
        let consumers = topology.local("consumers");

        let network_addr = network.addr();

        network.mount(elfo::batteries::network::new(&topology));
        configurers.mount(elfo::batteries::configurer::fixture(
            &topology,
            client_config(10000),
        ));
        consumers.mount(simple_consumer());

        Ok(elfo::_priv::do_start(topology, false, |ctx, _| async move {
            while let Some(new_port) = switch_rx.recv().await {
                debug!("switching to port {new_port}");

                let new_config = client_config(new_port);
                let network_config = new_config.get("system").and_then(|v| v.get("network"));

                ctx.try_send_to(
                    network_addr,
                    UpdateConfig::new(<_>::deserialize(network_config.unwrap().clone()).unwrap()),
                )
                .unwrap();
            }
        })
        .await?)
    });

    let mut steps = 5;
    let mut expected_src = 10000;

    while steps > 0 {
        sim.step().unwrap();

        // Switch between ports every time we see `SimpleMessage`.
        sim.links(|links| {
            for sent in links.flatten() {
                let (src, dst) = sent.pair();

                let got = matches!(sent.protocol(),
                    turmoil::Protocol::Tcp(turmoil::Segment::Data(_, bytes))
                    if bytes.windows(b"SimpleMessage".len()).any(|w| w == b"SimpleMessage")
                );

                if got {
                    steps -= 1;
                    warn!("SimpleMessage transferred from {src} to {dst}");
                    assert_eq!(src.port(), expected_src);
                    expected_src = if expected_src == 10000 { 11111 } else { 10000 };
                    switch_tx.send(expected_src).unwrap();
                }
            }
        });
    }
}

#[test]
fn listen_changed() {
    common::setup_logger();

    fn server_config(server_port: u16) -> toml::Value {
        let server_addr = format!("turmoil07://0.0.0.0:{server_port}");
        toml! {
            [system.network]
            listen = [server_addr]
            discovery.attempt_interval = "1s"
            ping_interval = "1s"
            idle_timeout = "1s"
        }
        .into()
    }

    let mut sim = turmoil_sim();

    let (switch_tx, switch_rx) = mpsc::unbounded_channel();
    let switch_rx = elfo::MoveOwnership::from(switch_rx);

    sim.host("server", move || {
        let mut switch_rx = switch_rx.take().unwrap();
        async move {
            let topology = Topology::empty();
            let configurers = topology.local("system.configurers").entrypoint();
            let network = topology.local("system.network");
            let producers = topology.local("producers");
            let consumers = topology.remote("consumers");

            let network_addr = network.addr();

            producers.route_to(&consumers, |_, _| topology::Outcome::Broadcast);

            network.mount(elfo::batteries::network::new(&topology));
            configurers.mount(elfo::batteries::configurer::fixture(
                &topology,
                server_config(10000),
            ));
            producers.mount(simple_producer());

            Ok(elfo::_priv::do_start(topology, false, |ctx, _| async move {
                while let Some(new_port) = switch_rx.recv().await {
                    debug!("switching to port {new_port}");

                    let new_config = server_config(new_port);
                    let network_config = new_config.get("system").and_then(|v| v.get("network"));

                    ctx.try_send_to(
                        network_addr,
                        UpdateConfig::new(
                            <_>::deserialize(network_config.unwrap().clone()).unwrap(),
                        ),
                    )
                    .unwrap();
                }
            })
            .await?)
        }
    });

    sim.client("client", async {
        let topology = Topology::empty();
        let configurers = topology.local("system.configurers").entrypoint();
        let network = topology.local("system.network");
        let consumers = topology.local("consumers");

        network.mount(elfo::batteries::network::new(&topology));
        configurers.mount(elfo::batteries::configurer::fixture(
            &topology,
            toml! {
                [system.network]
                discovery.predefined = [
                    "turmoil07://server:10000",
                    "turmoil07://server:11111",
                ]
                discovery.attempt_interval = "1s"
                ping_interval = "1s"
                idle_timeout = "1s"
            },
        ));
        consumers.mount(simple_consumer());

        Ok(elfo::init::try_start(topology).await?)
    });

    let mut steps = 5;
    let mut expected_src = 10000;

    while steps > 0 {
        sim.step().unwrap();

        // Switch between ports every time we see `SimpleMessage`.
        sim.links(|links| {
            for sent in links.flatten() {
                let (src, dst) = sent.pair();

                let got = matches!(sent.protocol(),
                    turmoil::Protocol::Tcp(turmoil::Segment::Data(_, bytes))
                    if bytes.windows(b"SimpleMessage".len()).any(|w| w == b"SimpleMessage")
                );

                if got {
                    steps -= 1;
                    warn!("SimpleMessage transferred from {src} to {dst}");
                    assert_eq!(src.port(), expected_src);
                    expected_src = if expected_src == 10000 { 11111 } else { 10000 };
                    switch_tx.send(expected_src).unwrap();
                }
            }
        });
    }
}
