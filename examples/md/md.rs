use std::time::Duration;

use elfo::{
    prelude::*,
    routers::{MapRouter, Outcome},
    time::Interval,
};

use crate::protocol::*;

pub fn new() -> Blueprint {
    ActorGroup::new()
        // Actors in this group are started on first `SubscribeToMd`.
        .router(MapRouter::new(|envelope| {
            msg!(match envelope {
                SubscribeToMd { market } => Outcome::Unicast(market.clone()),
                _ => Outcome::Default,
            })
        }))
        .exec(exec)
}

#[message]
struct TimerTick;

async fn exec(mut ctx: Context<(), Market>) {
    // Emulate some incoming data.
    let mut amt = 0.;
    ctx.attach(Interval::new(TimerTick))
        .start(Duration::from_secs(1));

    let mut subs = Vec::new();

    while let Some(envelope) = ctx.recv().await {
        let sender = envelope.sender();

        msg!(match envelope {
            // Ok, we got a new "subscription", let's remember it.
            // Send a snapshot and add the sender to the list of subscribers.
            (SubscribeToMd, token) => {
                subs.push(sender);
                tracing::info!("new subscription");

                ctx.respond(
                    token,
                    SubscribedToMd {
                        market: ctx.key().clone(),
                        snapshot: vec![MdLevel(1., 10.), MdLevel(2., 20.)],
                    },
                );
            }

            TimerTick => {
                amt += 1.0;
                let msg = MdUpdated {
                    market: ctx.key().clone(),
                    levels: vec![MdLevel(1., amt)],
                };

                for addr in &subs {
                    // TBD: handle errors here.
                    // For instance, "unsubscribe" if a strategy died.
                    let _ = ctx.try_send_to(*addr, msg.clone());
                }
            }
        })
    }
}
