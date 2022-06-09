//! How to attach alternative channels (e.g. `tokio::sync::broadcast`).
//! It's done by passing the channel via arguments.
//! Another way is to pass via a dedicated message.

use std::time::Duration;

use elfo::{
    config::AnyConfig,
    prelude::*,
    routers::{MapRouter, Outcome},
    stream::Stream,
    time::Interval,
};
use tokio::sync::broadcast;

#[message]
struct SomeMessage(u32);

#[message]
struct Lagged(u64);

fn receiver(broadcast_tx: broadcast::Sender<SomeMessage>) -> Schema {
    ActorGroup::new()
        .router(MapRouter::new(|_| Outcome::Multicast(vec![0, 1, 2])))
        .exec(move |ctx| {
            // Wrap into `elfo::Stream`.
            let mut broadcast_rx = broadcast_tx.subscribe();
            let stream = Stream::generate(|mut y| async move {
                use broadcast::error::RecvError;
                loop {
                    match broadcast_rx.recv().await {
                        Ok(msg) => y.emit(msg).await,
                        Err(RecvError::Lagged(skipped)) => y.emit(Lagged(skipped)).await,
                        Err(RecvError::Closed) => break,
                    }
                }
            });

            async move {
                // Attach the stream to the context.
                let mut ctx = ctx.with(stream);

                while let Some(envelope) = ctx.recv().await {
                    msg!(match envelope {
                        SomeMessage(num) => tracing::info!("got {}", num),
                        Lagged(skipped) => panic!("lost {} messages", skipped),
                    })
                }
            }
        })
}

fn sender(broadcast_tx: broadcast::Sender<SomeMessage>) -> Schema {
    #[message]
    struct SomeTick;

    ActorGroup::new().exec(move |ctx| {
        let broadcast_tx = broadcast_tx.clone();
        async move {
            let interval = Interval::new(|| SomeTick);
            interval.set_period(Duration::from_secs(1));
            let mut ctx = ctx.with(&interval);

            let mut num = 0;

            while let Some(envelope) = ctx.recv().await {
                msg!(match envelope {
                    SomeTick => {
                        let _ = broadcast_tx.send(SomeMessage(num));
                        num += 1;
                    }
                })
            }
        }
    })
}

#[tokio::main]
async fn main() {
    let topology = elfo::Topology::empty();
    let logger = elfo::logger::init();

    let senders = topology.local("senders");
    let receivers = topology.local("receivers");
    let loggers = topology.local("loggers");
    let configurers = topology.local("system.configurers").entrypoint();

    let (broadcast_tx, _rx) = broadcast::channel(42);

    senders.mount(self::sender(broadcast_tx.clone()));
    receivers.mount(self::receiver(broadcast_tx));
    loggers.mount(logger);
    configurers.mount(elfo::configurer::fixture(&topology, AnyConfig::default()));

    elfo::start(topology).await;
}
