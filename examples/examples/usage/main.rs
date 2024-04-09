// Let's build a simple application with three actor groups:
// * *producers* send some numbers to *aggregators*
// * *reporters* ask summaries from *aggregators*

//~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
//              protocol
//~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~

// All actor crates depend on one or more protocols.
// Dependencies between actors should be avoided.
mod protocol {
    use derive_more::{Display, From};
    use elfo::prelude::*;

    // It's just a regular message.
    // `message` derives
    // * `Debug` for logging in dev env
    // * `Serialize` and `Deserialize` for dumping and comminication between nodes
    // * `Message` and `Request` to restrict contracts
    #[message]
    pub struct AddNum {
        pub group: GroupId,
        pub num: u32,
    }

    // Messages with specified `ret` are requests.
    #[message(ret = Summary)]
    pub struct Summarize {
        pub group_filter: GroupFilter,
    }

    // Parts of messages can be marked with `message(part)`
    // to derive `Debug`, `Clone`, `Serialize` and `Deserialize`.
    #[message(part)]
    pub enum GroupFilter {
        All,
        ById(GroupId),
    }

    // Responses don't have to implement `Message`.
    #[message(part)]
    pub struct Summary {
        pub group: GroupId,
        pub sum: u32,
    }

    // Wrappers can be marked as `transparent`, that adds `serde(transparent)`
    // and implements `Debug` without printing the wrapper's name.
    #[message(part, transparent)]
    #[derive(Copy, PartialEq, Eq, Hash, From, Display)]
    pub struct GroupId(u32);
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
        group_count: u32,
        item_count: u32,
        // Wrap credentials to hide them in logs and dumps.
        #[serde(default)]
        password: Secret<String>,
    }

    // It's a group factory. The module can have a lot of them with different
    // arguments, just like constructors.
    pub fn new() -> Blueprint {
        ActorGroup::new()
            .config::<Config>()
            .exec(move |ctx| async move {
                // Use `ctx.config()` to get an actual version of the config.
                let item_count = ctx.config().item_count;
                let group_count = ctx.config().group_count;

                // Send some numbers.
                for num in 0..item_count {
                    let group = GroupId::from(num % group_count);

                    // `send().await` returns when the message is placed in a mailbox.
                    // ... or returns an error if all destinations are closed and cannot be
                    // restarted right now.
                    // Note that `elfo` logs warnings on its own and with restricted rate.
                    let _ = ctx.send(AddNum { group, num }).await;
                }

                // The supervisor will restart failed actors with back off mechanism.
                bail!("suicide");
            })
    }
}

//~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
//              aggregator
//~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~

// Another actor group with sharding.
mod aggregator {
    use elfo::{
        prelude::*,
        routers::{MapRouter, Outcome},
    };

    use crate::protocol::*;

    pub fn new() -> Blueprint {
        ActorGroup::new()
            // Routers are called on a sending side, potentially from many threads.
            // Usually, routers extract some sharding key from messages.
            //
            // See `MapRouter::with_state` for more complex routers with a state
            // (potentially depending on the config).
            .router(MapRouter::new(|envelope| {
                // `Envelope` is an abstract wrapper around message with some metadata.
                // Envelopes with known types are represented as `Envelope<T>`.
                //
                // It's not possible to mix different types in one `match`, thus
                // the special `msg!` macro should be used to beat it.
                // Reuse `match` syntax allows us to be compatible with `rustfmt`.
                msg!(match envelope {
                    // `Unicast` is for sending to only one specific actor.
                    // A new actor will be spawned if there is no actor for this key (`group`).
                    AddNum { group, .. } => Outcome::Unicast(*group),
                    // `Broadcast` is for sending to all already spawned actors.
                    Summarize {
                        group_filter: GroupFilter::All,
                        ..
                    } => Outcome::Broadcast,
                    Summarize {
                        group_filter: GroupFilter::ById(id),
                        ..
                    } => Outcome::Unicast(*id),
                    // Also there are other variants: `Multicast`, `Discard` and this one.
                    _ => Outcome::Default,
                })
            }))
            .exec(aggregator)
    }

    async fn aggregator(mut ctx: Context<(), GroupId>) {
        // Define some shard-specific state.
        let mut sum = 0;
        // Get a sharding key.
        let group = *ctx.key();

        // The main actor loop: receive a message, handle it, repeat.
        // Returns `None` and breaks the loop if actor's mailbox is closed
        // (usually when the system terminates).
        while let Some(envelope) = ctx.recv().await {
            msg!(match envelope {
                msg @ AddNum => {
                    sum += msg.num;
                }
                // It's a syntax for requests.
                // See more patterns in `elfo/tests/msg_macro.rs`.
                (Summarize, token) => {
                    // Use `token` to respond. The token cannot be used twice.
                    // If the token is dropped without responding,
                    // the sending side will get `RequestError::Ignored`.
                    ctx.respond(token, Summary { group, sum });
                }
            });
        }

        // Some work to perform a graceful termination.
    }
}

//~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
//               reporter
//~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~

mod reporter {
    use std::time::Duration;

    use elfo::{
        messages::{ConfigUpdated, ValidateConfig},
        prelude::*,
        time::Interval,
    };
    use metrics::increment_counter;
    use serde::{Deserialize, Serialize};

    use crate::protocol::*;

    #[derive(Debug, Serialize, Deserialize)]
    struct Config {
        #[serde(with = "humantime_serde")]
        interval: Duration,
    }

    // Sometimes it's useful to define private messages.
    #[message]
    struct SummarizeTick;

    pub fn new() -> Blueprint {
        ActorGroup::new().config::<Config>().exec(reporter)
    }

    async fn reporter(mut ctx: Context<Config>) {
        // It's possible to attach additional sources to handle everything the same way.
        let interval = ctx.attach(Interval::new(SummarizeTick));
        // Some of them should be configured and started.
        interval.start(ctx.config().interval);

        while let Some(envelope) = ctx.recv().await {
            msg!(match envelope {
                (ValidateConfig { config, .. }, token) => {
                    // You can additionally validate a config against dynamic data.
                    // If all actors pass or ignore the validation step,
                    // configs are updated (`ConfigUpdated` event).
                    let _config = ctx.unpack_config(&config);
                    // Accept the update.
                    ctx.respond(token, Ok(()));
                    // Reject the update.
                    // ctx.respond(token, Err("oops".into()));
                }
                ConfigUpdated => {
                    // Update sources, e.g. the active interval.
                    interval.set_period(ctx.config().interval);

                    // Sometimes config updates require more complex actions,
                    // e.g. reopen connections. Do it here.
                }
                SummarizeTick => {
                    // `request(..).resolve().await` returns the result
                    // ... or with error, if something went wrong.
                    // In the future, `request(..).id().await` will be able to be used
                    // in order to get a response via the mailbox.
                    let req = Summarize {
                        group_filter: GroupFilter::All,
                    };
                    for summary in ctx.request(req).all().resolve().await {
                        tracing::info!(?summary, "summary");
                    }

                    // How to use metrics.
                    increment_counter!("ticks_total");
                }
            });
        }
    }
}

//~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
//               topology
//~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~

// Topology definition with actor groups and connections between them.
fn topology() -> elfo::Topology {
    let topology = elfo::Topology::empty();

    // Set up logging (based on the `tracing` crate).
    // `elfo` provides a logger actor group to support runtime control.
    //
    // Also, you can filter logs by passing `RUST_LOG`:
    // * `RUST_LOG=elfo`
    // * `RUST_LOG=info,[{actor_group=aggregators}]`
    //
    // However, it's more useful to control logging in the config file.
    let logger = elfo::batteries::logger::init();
    // Setup up telemetry (based on the `metrics` crate).
    let telemeter = elfo::batteries::telemeter::init();

    // Define actor groups.
    let producers = topology.local("producers");
    let aggregators = topology.local("aggregators");
    let reporters = topology.local("reporters");
    let loggers = topology.local("system.loggers");
    let telemeters = topology.local("system.telemeters");
    let dumpers = topology.local("system.dumpers");
    let pingers = topology.local("system.pingers");
    let configurers = topology.local("system.configurers").entrypoint();

    // Define links between actor groups.
    // Producers send raw data to aggregators.
    producers.route_all_to(&aggregators);
    // Reporters ask aggregators for a summary.
    reporters.route_all_to(&aggregators);

    // Mount specific implementations.
    producers.mount(producer::new());
    aggregators.mount(aggregator::new());
    reporters.mount(reporter::new());
    loggers.mount(logger);
    telemeters.mount(telemeter);
    dumpers.mount(elfo::batteries::dumper::new());
    pingers.mount(elfo::batteries::pinger::new(&topology));

    // Actors can use `topology` as an extended service locator.
    // Usually it should be used for utilities only.
    let config_path = "examples/examples/usage/config.toml";
    configurers.mount(elfo::batteries::configurer::from_path(
        &topology,
        config_path,
    ));

    topology
}

//~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
//                setup
//~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~

// Optionally, the allocation statistics can be also enabled.
#[cfg(feature = "unstable")] // TODO: stabilize it
#[global_allocator]
static ALLOCATOR: elfo_telemeter::AllocatorStats<std::alloc::System> =
    elfo_telemeter::AllocatorStats::new(std::alloc::System);

#[tokio::main]
async fn main() {
    elfo::init::start(topology()).await;
}
