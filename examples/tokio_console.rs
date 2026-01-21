//! How to enable `tokio-console` support.

use std::{env, time::Duration};

use elfo::{config::AnyConfig, prelude::*, time::Interval};
use tracing_subscriber::{layer::SubscriberExt as _, prelude::*, EnvFilter};

#[message]
struct Tick;

fn counter() -> Blueprint {
    ActorGroup::new().exec(|mut ctx| async move {
        let mut count = 0;
        let interval = ctx.attach(Interval::new(Tick));
        interval.start(Duration::from_secs(1));

        while let Some(envelope) = ctx.recv().await {
            msg!(match envelope {
                Tick => {
                    count += 1;
                    tracing::info!("count: {}", count);
                }
            })
        }
    })
}

#[tokio::main]
async fn main() {
    let topology = elfo::Topology::empty();

    let logger = {
        let (logger, filter, transmit) = elfo::batteries::logger::new();

        let env_filter = env::var(EnvFilter::DEFAULT_ENV)
            .ok()
            .map(|_| EnvFilter::try_from_default_env().expect("invalid env"));

        tracing_subscriber::registry()
            .with(console_subscriber::spawn())
            .with(transmit.with_filter(filter).with_filter(env_filter))
            .init();

        logger
    };

    let counters = topology.local("counters");
    let loggers = topology.local("system.loggers");
    let configurers = topology.local("system.configurers").entrypoint();

    counters.mount(counter());
    loggers.mount(logger);

    configurers.mount(elfo::batteries::configurer::fixture(
        &topology,
        AnyConfig::default(),
    ));

    elfo::init::start(topology).await;
}
