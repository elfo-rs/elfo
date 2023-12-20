#![cfg(feature = "test-util")]

use std::{num::NonZeroU64, time::Duration};

use elfo::{
    messages::UpdateConfig,
    prelude::*,
    routers::{MapRouter, Outcome, Singleton},
    RestartParams, RestartPolicy,
};

#[tokio::test]
async fn it_works() {
    #[message]
    struct Start;

    #[message]
    struct Restarted;
    #[message]
    struct GroupMounted;
    #[message]
    struct OnMessage;

    let blueprint = ActorGroup::new()
        .router(MapRouter::new(|e| {
            msg!(match e {
                UpdateConfig | Start => Outcome::Unicast(Singleton),
                _ => Outcome::Discard,
            })
        }))
        .restart_policy(RestartPolicy::on_failure(
            RestartParams::new(Duration::from_secs(0), Duration::from_secs(0))
                .auto_reset(Duration::MAX)
                .max_retries(NonZeroU64::new(1).unwrap()),
        ))
        .exec(move |ctx| async move {
            if ctx.start_info().cause.is_group_mounted() {
                let _ = ctx.send(GroupMounted).await;
            }
            if ctx.start_info().cause.is_restarted() {
                let _ = ctx.send(Restarted).await;
            }
            if ctx.start_info().cause.is_on_message() {
                let _ = ctx.send(OnMessage).await;
            }
            panic!("boom!");
        });

    let mut proxy = elfo::test::proxy(blueprint, elfo::config::AnyConfig::default()).await;
    assert_msg!(proxy.recv().await, GroupMounted);
    assert_msg!(proxy.recv().await, Restarted);
    proxy.send(Start).await;
    assert_msg!(proxy.recv().await, OnMessage);
    assert_msg!(proxy.recv().await, Restarted);
}
