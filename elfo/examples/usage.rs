//~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
//              protocol
//~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~

mod protocol {
    use elfo::prelude::*;

    #[message]
    pub struct AddNum {
        pub group: u32,
        pub num: u32,
    }

    #[message]
    pub struct AddNum2 {
        pub group: u32,
        pub num: u32,
    }

    #[message(ret = Report)]
    pub struct Summarize {
        pub group: u32,
    }

    #[message]
    pub struct Report(pub u32);
}

//~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
//              producer
//~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~

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
                    let _ = ctx.send(msg).await;
                }

                // Ask every group.
                for &group in &[0, 1, 2] {
                    let _ = ctx.request(Summarize { group }).resolve().await;
                }

                bail!("suicide");
            })
    }
}

//~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
//              summator
//~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~

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
            .router(MapRouter::new(|envelope| {
                msg!(match envelope {
                    AddNum { group, .. } => Outcome::Unicast(*group),
                    Summarize { group, .. } => Outcome::Unicast(*group),
                    _ => Outcome::Default,
                })
            }))
            .exec(summator)
    }

    #[message]
    struct TimerTick;

    async fn summator(ctx: Context<Config, u32>) {
        let mut sum = 0;

        let interval = Interval::new(|| TimerTick);

        let mut ctx = ctx.with(&interval);

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

    let topology = elfo::Topology::empty();

    let producers = topology.local("producers");
    let summators = topology.local("summators");
    let configurers = topology.local("system.configurers").entrypoint();

    producers.route_all_to(&summators);

    producers.mount(producer::new());
    summators.mount(summator::new());

    let config_path = "elfo/examples/config.toml";
    configurers.mount(elfo::actors::configurer::from_path(&topology, config_path));

    elfo::start(topology).await;
}
