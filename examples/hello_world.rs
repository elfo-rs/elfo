mod protocol {
    use elfo::prelude::*;

    // A plain fire-and-forget command.
    // `#[message]` derives elfo::Message, Debug, Clone, Serialize, Deserialize.
    #[message]
    pub struct AddNum {
        pub num: u32,
    }

    // A request: `ret = u64` means the sender expects a `u64` back.
    #[message(ret = u32)]
    pub struct GetSum;
}

mod producer {
    use elfo::prelude::*;

    use crate::protocol::*;

    pub fn new() -> Blueprint {
        ActorGroup::new().exec(|ctx| async move {
            // Send numbers.
            for num in 0..10u32 {
                let _ = ctx.send(AddNum { num }).await;
            }

            // Ask the aggregator for the current sum and wait for the reply.
            match ctx.request(GetSum).resolve().await {
                Ok(sum) => tracing::info!(sum, "done"),
                Err(err) => tracing::error!(%err, "request failed"),
            }
        })
    }
}

mod aggregator {
    use elfo::prelude::*;

    use crate::protocol::*;

    pub fn new() -> Blueprint {
        ActorGroup::new().exec(|mut ctx| async move {
            let mut sum = 0u32;

            // The main actor loop: receive a message, handle it, repeat.
            // Returns `None` (breaking the loop) when the mailbox is closed.
            while let Some(envelope) = ctx.recv().await {
                msg!(match envelope {
                    AddNum { num } => {
                        sum += num;
                    }
                    // The `(Request, token)` pattern handles a request-response pair.
                    (GetSum, token) => {
                        ctx.respond(token, sum);
                    }
                });
            }
        })
    }
}

fn topology() -> elfo::Topology {
    let topology = elfo::Topology::empty();

    // Set up built-in actors (logging, config distribution).
    let logger = elfo::batteries::logger::init();
    let loggers = topology.local("system.loggers");
    let configurers = topology.local("system.configurers").entrypoint();

    // Declare your own groups.
    let producers = topology.local("producers");
    let aggregators = topology.local("aggregators");

    // Messages sent by the producer are forwarded to the aggregator.
    producers.route_all_to(&aggregators);

    // Bind blueprints to groups.
    producers.mount(producer::new());
    aggregators.mount(aggregator::new());
    loggers.mount(logger);
    configurers.mount(elfo::batteries::configurer::fixture(
        &topology,
        elfo::config::AnyConfig::default(),
    ));

    topology
}

#[tokio::main]
async fn main() {
    elfo::init::start(topology()).await;
}
