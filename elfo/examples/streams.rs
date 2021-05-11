use elfo::{config::AnyConfig, prelude::*, stream};

#[message]
struct SomeMessage(u32);

#[message]
struct EndOfMessages;

fn samples() -> Schema {
    ActorGroup::new().exec(|ctx| async move {
        let stream1 =
            stream::Stream::new(futures::stream::iter(vec![SomeMessage(0), SomeMessage(1)]));

        let mut ctx = ctx.with(stream1);

        while let Some(_envelope) = ctx.recv().await {
            // ...
        }
    })
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_target(false)
        .init();

    let topology = elfo::Topology::empty();
    let samples = topology.local("samples");
    let configurers = topology.local("system.configurers").entrypoint();

    samples.mount(self::samples());
    configurers.mount(elfo::configurer::fixture(&topology, AnyConfig::default()));

    elfo::start(topology).await;
}
