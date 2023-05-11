use elfo::{config::AnyConfig, prelude::*, stream::Stream};
use futures::stream;

#[message]
struct SomeMessage(u32);

#[message]
struct EndOfMessages;

fn samples() -> Blueprint {
    ActorGroup::new().exec(|mut ctx| async move {
        let stream = Stream::from_futures03(stream::iter(vec![SomeMessage(0), SomeMessage(1)]));
        ctx.attach(stream);

        while let Some(_envelope) = ctx.recv().await {
            // ...
        }
    })
}

#[tokio::main]
async fn main() {
    let topology = elfo::Topology::empty();
    let logger = elfo::batteries::logger::init();

    let samples = topology.local("samples");
    let loggers = topology.local("loggers");
    let configurers = topology.local("system.configurers").entrypoint();

    samples.mount(self::samples());
    loggers.mount(logger);
    configurers.mount(elfo::batteries::configurer::fixture(
        &topology,
        AnyConfig::default(),
    ));

    elfo::start(topology).await;
}
