use elfo::{
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
            if let Ok(report) = ctx.request(Summarize { group }).await {
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
                _ => Outcome::Discard,
            })
        }))
        .exec(|ctx| async move {
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
                    Terminate => break,
                    _ => {}
                });
            }

            Err("some error")
        })
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .init();

    let topology = elfo::Topology::default();

    let producers = topology.local("producers");
    let summators = topology.local("summators");
    // let informers = topology.remote("informers");

    // producers.route_to(&summators, |e| {
    // msg!(match e {
    // AddNum { .. } | Summarize { .. } | Terminate => true,
    //_ => false,
    //});

    producers.route_to(&summators, |_e| true);

    // (&producers + &summators).route_all_to(&informers);

    summators.mount(self::summators(), None::<Report>).await;
    producers.mount(self::producers(), Some(Report(2))).await;

    tokio::time::sleep(tokio::time::Duration::from_secs(10)).await;
}
