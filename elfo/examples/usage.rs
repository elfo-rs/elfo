use elfo::{
    prelude::*,
    routers::{MapRouter, Outcome},
};

#[message]
struct AddNum {
    group: u32,
    num: u32,
}

#[message(response(Report))]
struct Summarize {
    group: u32,
}
#[message]
struct Report(u32);

#[message]
struct Terminate;

fn producer(ctx: &Context, dest: Addr) -> Addr {
    ActorGroup::new()
        .name("producer")
        .exec(move |ctx: Context<(), ()>| async move {
            // Send some numbers.
            for i in 0..50 {
                let msg = AddNum {
                    group: i % 3,
                    num: i,
                };
                let _ = ctx.send_to(dest, msg).await;
                println!("sent {}", i);
            }

            // Ask every group.
            for &group in &[0, 1, 2] {
                if let Ok(report) = ctx.ask(dest, Summarize { group }).await {
                    println!("group={} sum={}", group, report.0);
                }
            }

            // Terminate everything.
            let _ = ctx.send_to(dest, Terminate).await;
        })
        .spawn(ctx)
}

fn summator(ctx: &Context) -> Addr {
    ActorGroup::new()
        .name("summator")
        .router(MapRouter::new(|envelope| {
            msg!(match envelope {
                AddNum { group, .. } => Outcome::Unicast(*group),
                Summarize { group, .. } => Outcome::Unicast(*group),
                Terminate => Outcome::Broadcast,
                _ => Outcome::Discard,
            })
        }))
        .exec(|ctx: Context<(), u32>| async move {
            let mut sum = 0;

            while let Some(envelope) = ctx.recv().await {
                msg!(match envelope {
                    msg @ AddNum { .. } => {
                        println!("got {}", msg.num);
                        sum += msg.num;
                    }
                    (Summarize { .. }, token) => {
                        let _ = ctx.reply(token, Report(sum));
                    }
                    Terminate => break,
                    _ => {}
                });
            }

            Err("some error")
        })
        .spawn(ctx)
}

#[tokio::main]
async fn main() {
    let ctx = Context::root();

    println!("system started");
    let summator = summator(&ctx);
    let producer = producer(&ctx, summator);
    // let _ = ctx.send_to(producer, ()).await;
    // core.wait_all().await;
    println!("everything stopped");
}
