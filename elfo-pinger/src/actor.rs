use std::time::Duration;

use tokio::{select, time};
use tracing::{debug, info, warn};

use elfo_core::{
    message, messages::Ping, scope, time::Interval, topology::ActorGroup, ActorStatus, Addr,
    Context, Topology,
};
use elfo_utils::ward;

use crate::config::Config;

#[message]
struct PingTick;

pub(crate) async fn exec(mut ctx: Context<Config>, topology: Topology) {
    let interval = ctx.attach(Interval::new(PingTick));

    let mut groups = collect_groups(&topology, &[ctx.group()]);
    let group_count = groups.len() as u32;

    if groups.is_empty() {
        info!("no groups to ping, terminating");
        return;
    }

    let mut is_alarming = false;
    let mut timed_out = 0;
    let mut pinging = None;

    interval.start(ctx.config().ping_interval / group_count);

    // Accept envelopes from the mailbox concurrently with pinging
    // in order to avoid getting stuck with the configurer.
    // TODO: replace it with async requests in the future.
    loop {
        select! {
            envelope = ctx.recv() => {
                let envelope = ward!(envelope, break);
                interval.set_period(ctx.config().ping_interval / group_count);

                if !envelope.is::<PingTick>() || pinging.is_some() {
                    continue;
                }

                if groups.is_empty() {
                    if timed_out == 0 && is_alarming {
                        is_alarming = false;
                        ctx.set_status(ActorStatus::NORMAL);
                    }

                    groups = collect_groups(&topology, &[ctx.group()]);
                    timed_out = 0;
                }

                let group = groups.pop().unwrap();
                let warn_threshold = ctx.config().warn_threshold;

                // Expose a current scope to preserve an original trace id.
                let fut = scope::expose().within(ping_group(ctx.pruned(), group, warn_threshold));
                pinging = Some(Box::pin(fut));
            },
            responsive = async { pinging.as_mut().unwrap().await }, if pinging.is_some() => {
                pinging = None;

                if !responsive {
                    timed_out += 1;

                    if !is_alarming {
                        is_alarming = true;
                        ctx.set_status(ActorStatus::ALARMING);
                    }
                }
            },
        }
    }
}

fn collect_groups(topology: &Topology, exclude: &[Addr]) -> Vec<ActorGroup> {
    topology
        .actor_groups()
        .filter(|group| !exclude.contains(&group.addr))
        .collect()
}

async fn ping_group(ctx: Context, group: ActorGroup, warn_threshold: Duration) -> bool {
    debug!(group = %group.name, "checking a group");
    let fut = ctx.request_to(group.addr, Ping).all().resolve();
    if time::timeout(warn_threshold, fut).await.is_err() {
        warn!(
            message = "group hasn't responded in the allowed time",
            group = %group.name,
            timeout = ?warn_threshold,
        );
        false
    } else {
        true
    }
}
