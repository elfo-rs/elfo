use anyhow::bail;

// Just adds `ActorGroup`, `Context`, `Schema` and macros.
use elfo::prelude::*;
use serde::{Deserialize, Serialize};

use elfo::{
    actors::configurers,
    config::Secret,
    messages::ValidateConfig,
    routers::{MapRouter, Outcome},
};

//~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
//              protocol
//~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~

#[message]
struct AddNum {
    group: u32,
    num: u32,
}

#[message(ret = Report)]
struct Summarize {
    group: u32,
}

#[message]
struct Report(u32);

//~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
//              producer
//~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~

#[derive(Serialize, Deserialize)]
struct Config {
    count: u32,
    // Wrap credentials to hide them in logs and dumps.
    #[serde(default)]
    password: Secret<String>,
}

fn producers() -> Schema {
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

//~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
//              summator
//~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~

fn summators() -> Schema {
    ActorGroup::new()
        .router(MapRouter::new(|envelope| {
            msg!(match envelope {
                AddNum { group, .. } => Outcome::Unicast(*group),
                Summarize { group, .. } => Outcome::Unicast(*group),
                _ => Outcome::Default,
            })
        }))
        .exec(summator)
}

async fn summator(mut ctx: Context<(), u32>) {
    let mut sum = 0;

    while let Some(envelope) = ctx.recv().await {
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
            _ => {}
        });
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

    producers.mount(self::producers());
    summators.mount(self::summators());

    let config_path = "elfo/examples/config.toml";

    configurers.mount(self::configurers(&topology, config_path).expect("invalid config"));

    elfo::start(topology).await;
}
