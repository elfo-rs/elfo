use tokio::time;
use tracing::{debug, warn};

use elfo_core as elfo;
use elfo_macros::{message, msg_raw as msg};

use elfo::{messages::Ping, time::Interval, ActorStatus, Context, Topology};

use crate::config::Config;

#[message(elfo = elfo_core)]
struct PingTick;

pub(crate) async fn exec(ctx: Context<Config>, topology: Topology) {
    let interval = Interval::new(|| PingTick);
    let mut ctx = ctx.with(&interval);

    interval.set_period(ctx.config().ping_interval);

    let mut timed_out = 0;

    while let Some(envelope) = ctx.recv().await {
        interval.set_period(ctx.config().ping_interval);

        msg!(match envelope {
            PingTick => {
                // To avoid accumulating multiple ticks.
                // TODO: alter the interval's behaviour instead.
                interval.reset();

                let mut new_timed_out = 0;

                for group in topology.actor_groups().filter(|g| g.addr != ctx.group()) {
                    debug!(group = %group.name, "checking a group");
                    let fut = ctx.request_to(group.addr, Ping).all().resolve();
                    let timeout = ctx.config().warn_threshold;
                    if time::timeout(timeout, fut).await.is_err() {
                        warn!(
                            message = "group hasn't responded in the allowed time",
                            group = %group.name,
                            timeout = ?timeout,
                        );
                        new_timed_out += 1;
                    }
                }

                debug!("all groups are checked");

                if new_timed_out > 0 {
                    timed_out = new_timed_out;
                    ctx.set_status(ActorStatus::ALARMING);
                } else if timed_out > 0 {
                    ctx.set_status(ActorStatus::NORMAL);
                }
            }
        });
    }
}
