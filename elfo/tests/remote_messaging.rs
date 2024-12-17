#![allow(missing_docs)]
#![cfg(feature = "network")]
#![cfg(feature = "turmoil06")]

use std::{sync::Arc, time::Duration};

use tokio::sync::Notify;
use toml::toml;
use tracing::info;

use elfo::{prelude::*, time::Interval, topology, Topology};

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
