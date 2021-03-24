use anyhow::{anyhow, Result};
use elfo::{
    messages::ValidateConfig,
    prelude::*,
    routers::{MapRouter, Outcome},
};
use tracing::info;

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

#[message]
struct Terminate;

fn producers() -> Schema {
    ActorGroup::new().exec(move |ctx| async move {
        // Send some numbers.
        for i in 0..50 {
            let msg = AddNum {
                group: i % 3,
                num: i,
            };
            let _ = ctx.send(msg).await;
            info!(%i, "sent");
        }

        // Ask every group.
        for &group in &[0, 1, 2] {
            if let Ok(report) = ctx.request(Summarize { group }).resolve().await {
                info!(group, sum = report.0, "asked");
            }
        }

        // Terminate everything.
        let _ = ctx.send(Terminate).await;
    })
}

fn summators() -> Schema {
    ActorGroup::new()
        .router(MapRouter::new(|envelope| {
            msg!(match envelope {
                AddNum { group, .. } => Outcome::Unicast(*group),
                Summarize { group, .. } => {
                    Outcome::Unicast(*group)
                }
                Terminate => Outcome::Broadcast,
                _ => Outcome::Default,
            })
        }))
        .exec(summator)
}

async fn summator(mut ctx: Context<(), u32>) -> Result<()> {
    let mut sum = 0;

    while let Some(envelope) = ctx.recv().await {
        msg!(match envelope {
            msg @ AddNum { .. } => {
                info!(num = msg.num, "got");
                sum += msg.num;
            }
            (Summarize { .. }, token) => {
                let _ = ctx.respond(token, Report(sum));
            }
            (ValidateConfig { config, .. }, token) => {
                let _config = ctx.unpack_config(&config);
                let _ = ctx.respond(token, Err("oops".into()));
            }
            Terminate => break,
            _ => {}
        });
    }

    Err(anyhow!("oops"))
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .init();

    let topology = elfo::Topology::empty();

    let producers = topology.local("producers");
    let summators = topology.local("summators");
    let configurers = topology.local("system.configurers");

    producers.route_all_to(&summators);

    producers.mount(self::producers());
    summators.mount(self::summators());

    let config_path = "elfo/examples/config.toml";

    configurers.mount(elfo::actors::configurers(&topology, config_path).expect("invalid config"));

    elfo::start(topology).await;
}
