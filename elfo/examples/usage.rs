//~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
//              protocol
//~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~

// All actor crates depend on one or more protocols.
// Dependencies between actors should be avoided.
mod protocol {
    use elfo::prelude::*;

    // It's just a regular message.
    // `message` derives
    // * `Debug` for logging in dev env
    // * `Serialize` and `Deserialize` for dumping and comminication between nodes
    // * `Message` and `Request` to restrict contracts
    #[message]
    pub struct AddNum {
        pub group: u32,
        pub num: u32,
    }

    // Messages with specified `ret` are requests.
    #[message(ret = Report)]
    pub struct Summarize {
        pub group: u32,
    }

    // Actually, responses don't have to implement `Message`.
    #[message]
    pub struct Report(pub u32);
}

//~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
//              producer
//~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~

// An actor group with only one child so-called a singleton.
mod producer {
    use anyhow::bail;
    use elfo::{config::Secret, prelude::*};
    use serde::{Deserialize, Serialize};

    use crate::protocol::*;

    #[derive(Debug, Serialize, Deserialize)]
    struct Config {
        count: u32,
        // Wrap credentials to hide them in logs and dumps.
        #[serde(default)]
        password: Secret<String>,
    }

    // It's a group factory. The module can have a lot of them with different
    // arguments, just like constructors.
    pub fn new() -> Schema {
        ActorGroup::new()
            .config::<Config>()
            .exec(move |ctx| async move {
                let count = ctx.config().count;

                // Send some numbers.
                for i in 0..count {
                    let msg = AddNum {
                        group: i % 3,
                        num: i,
                    };

                    // `send().await` returns when the message is placed in a mailbox.
                    // ... or returns an error if all destinations are closed and cannot be
                    // restarted right now.
                    let _ = ctx.send(msg).await;
                }

                // Ask every group.
                for &group in &[0, 1, 2] {
                    // `request(..).resolve().await` returns the result
                    // ... or with error, if something went wrong.
                    // `request(..).id().await` can be used to get a response via the mailbox.
                    let _ = ctx.request(Summarize { group }).resolve().await;
                }

                // The supervisor will restart failed actor with back off mechanism.
                bail!("suicide");
            })
    }
}

//~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
//              summator
//~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~

// A more actor group that has sharding.
mod summator {
    use std::time::Duration;

    use elfo::{
        messages::ValidateConfig,
        prelude::*,
        routers::{MapRouter, Outcome},
        time::Interval,
    };
    use serde::{Deserialize, Serialize};

    use crate::protocol::*;

    #[derive(Debug, Serialize, Deserialize)]
    struct Config {
        #[serde(with = "humantime_serde")]
        interval: Duration,
    }

    pub fn new() -> Schema {
        ActorGroup::new()
            .config::<Config>()
            // Routers are called on a sending side, potentially from many threads.
            // Usually, routers extract some sharding key from messages.
            //
            // See `MapRouter::with_state` for more complex routers with a state
            // (potentially depending on the config).
            .router(MapRouter::new(|envelope| {
                msg!(match envelope {
                    AddNum { group, .. } => Outcome::Unicast(*group),
                    Summarize { group, .. } => Outcome::Unicast(*group),
                    _ => Outcome::Default,
                })
            }))
            .exec(summator)
    }

    // Sometimes it's useful to define private messages.
    #[message]
    struct TimerTick;

    async fn summator(ctx: Context<Config, u32>) {
        // Define some state.
        let mut sum = 0;

        let interval = Interval::new(|| TimerTick);

        // It's possible to attach additional sources to handle everything the same way.
        let mut ctx = ctx.with(&interval);

        // The main actor loop: receive a message, handle it, repeat.
        // Returns `None` and breaks the loop if actor's mailbox is closed
        // (usually when the system terminates).
        while let Some(envelope) = ctx.recv().await {
            interval.set_period(ctx.config().interval);

            msg!(match envelope {
                msg @ AddNum { .. } => {
                    sum += msg.num;
                }
                (Summarize { .. }, token) => {
                    let _ = ctx.respond(token, Report(sum));
                }
                (ValidateConfig { config, .. }, token) => {
                    // You can additionally validate a config against dynamic data.
                    // If all actors pass or ignore the validation step,
                    // configs are updated (`ConfigUpdated` event).
                    let _config = ctx.unpack_config(&config);
                    let _ = ctx.respond(token, Err("oops".into()));
                }
                TimerTick => tracing::info!("hello!"),
            });
        }
    }
}

//~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
//                setup
//~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_target(false)
        .init();

    // Let's define our topology with actor groups and connections between them.
    let topology = elfo::Topology::empty();

    let producers = topology.local("producers");
    let summators = topology.local("summators");
    let configurers = topology.local("system.configurers").entrypoint();

    // Define links between actor groups.
    producers.route_all_to(&summators);

    producers.mount(producer::new());
    summators.mount(summator::new());

    // Actors can use `topology` as extended service locator.
    // Usually it should be used for utilities only.
    let config_path = "elfo/examples/config.toml";
    configurers.mount(elfo::configurer::from_path(&topology, config_path));

    // Run actors.
    elfo::start(topology).await;
}
