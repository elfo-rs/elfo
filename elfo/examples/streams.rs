#![cfg(feature = "full")]

use elfo::{config::AnyConfig, prelude::*, stream::Stream};
use futures::stream;

#[message]
struct SomeMessage(u32);

#[message]
struct EndOfMessages;

fn samples() -> Schema {
    ActorGroup::new().exec(|ctx| async move {
        let stream1 = Stream::new(stream::iter(vec![SomeMessage(0), SomeMessage(1)]));

        let mut ctx = ctx.with(stream1);

        while let Some(_envelope) = ctx.recv().await {
            // ...
        }
    })
}

#[tokio::main]
async fn main() {
    let topology = elfo::Topology::empty();
    let logger = elfo::logger::init();

    let samples = topology.local("samples");
    let loggers = topology.local("loggers");
    let configurers = topology.local("system.configurers").entrypoint();

    samples.mount(self::samples());
    loggers.mount(logger);
    configurers.mount(elfo::configurer::fixture(&topology, AnyConfig::default()));

    elfo::start(topology).await;
}
