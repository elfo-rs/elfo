#![allow(missing_docs)]
#![cfg(feature = "network")]
#![cfg(feature = "turmoil06")]

use std::{sync::Arc, time::Duration};

use tokio::sync::Notify;
use toml::toml;
use tracing::{debug, info};

use elfo::{
    messages::UpdateConfig,
    prelude::*,
    routers::{MapRouter, Outcome, Singleton},
    stream::Stream,
    time::Interval,
    topology, Topology,
};

mod common;

#[test]
fn simple() {
    common::setup_logger();

    #[message]
    struct SimpleMessage(u64);

    #[message]
    struct ProducerTick;

    fn producer() -> Blueprint {
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

    let mut sim = turmoil::Builder::new()
        // TODO: We don't actually use I/O, but otherwise the test panics with:
        //  "there is no signal driver running"
        // Need to detect availability of the signal driver in the `Signal` source.
        .enable_tokio_io()
        .tick_duration(Duration::from_millis(100))
        .build();

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
                listen = ["turmoil06://0.0.0.0"]
                ping_interval = "1s"
                idle_timeout = "1s"
            },
        ));
        producers.mount(producer());

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
                discovery.predefined = ["turmoil06://server"]
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
                listen = ["turmoil06://0.0.0.0"]
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
                discovery.predefined = ["turmoil06://server"]
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
