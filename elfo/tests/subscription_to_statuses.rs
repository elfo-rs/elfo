#![allow(missing_docs)]
#![cfg(feature = "test-util")]

use std::time::Duration;

use elfo::{
    messages::{ActorStatusReport, SubscribeToActorStatuses},
    prelude::*,
    routers::{MapRouter, Outcome},
    test::Proxy,
    ActorStatus, ActorStatusKind,
};
use elfo_core::{RestartParams, RestartPolicy};

#[message]
struct Start(u32);

#[message]
struct Stop(u32);

#[message]
struct Fail(u32);

async fn run_group() -> Proxy {
    let blueprint = ActorGroup::new()
        .router(MapRouter::new(|envelope| {
            msg!(match envelope {
                Start(n) | Stop(n) | Fail(n) => Outcome::Unicast(*n),
                _ => Outcome::Default,
            })
        }))
        .restart_policy(RestartPolicy::on_failure(RestartParams::new(
            Duration::from_secs(5),
            Duration::from_secs(30),
        )))
        .exec(move |mut ctx| async move {
            while let Some(envelope) = ctx.recv().await {
                msg!(match envelope {
                    Start => ctx.set_status(ActorStatus::NORMAL.with_details("on Start")),
                    Stop => {
                        ctx.set_status(ActorStatus::NORMAL.with_details("on Stop"));
                        break;
                    }
                    Fail => {
                        ctx.set_status(ActorStatus::NORMAL.with_details("on Fail"));
                        panic!("oops");
                    }
                    _ => unreachable!(),
                });
            }
        });

    elfo::test::proxy(blueprint, elfo::config::AnyConfig::default()).await
}

async fn check_seq(proxy: &mut Proxy, expected: &[(u32, ActorStatusKind, Option<&str>)]) {
    let mut actual = Vec::new();
    while let Some(envelope) = proxy.try_recv().await {
        msg!(match envelope {
            ActorStatusReport { meta, status, .. } => {
                assert_eq!(meta.group, "subject");
                actual.push((meta, status));
            }
            _ => unreachable!(),
        })
    }

    // Sort messages to make tests deterministic.
    actual.sort_by_key(|(meta, _)| meta.key.clone());

    let actual = actual
        .into_iter()
        .map(|(meta, status)| format!("{} {:?} {:?}", meta.key, status.kind(), status.details()))
        .collect::<Vec<_>>();

    let expected = expected
        .iter()
        .map(|(key, status, details)| format!("{key} {status:?} {details:?}"))
        .collect::<Vec<_>>();

    assert_eq!(actual, expected);
}

#[tokio::test(start_paused = true)]
async fn it_works() {
    use ActorStatusKind::*;

    let mut proxy = run_group().await;

    // Start some actors before subscribing.
    proxy.send(Start(1)).await;
    proxy.send(Start(2)).await;
    proxy.send(Start(3)).await;
    proxy.send(Start(4)).await;

    // And stop/fail some of them.
    proxy.send(Stop(2)).await;
    proxy.send(Fail(3)).await;

    proxy.sync().await;

    // Now subscribe to status reports.
    proxy.send(SubscribeToActorStatuses::default()).await;
    proxy.sync().await;

    // Firstly, we should receive statuses of unstopped actors.
    check_seq(
        &mut proxy,
        &[
            (1, Normal, Some("on Start")),
            // There is no status for `2` because it's terminated.
            (3, Failed, Some("panic: oops")),
            (4, Normal, Some("on Start")),
        ],
    )
    .await;

    // Start another one.
    proxy.send(Start(5)).await;
    proxy.sync().await;

    check_seq(
        &mut proxy,
        &[
            (5, Initializing, None),
            (5, Normal, None),
            (5, Normal, Some("on Start")),
        ],
    )
    .await;

    // Repeating subscription should not produce any messages.
    proxy.send(SubscribeToActorStatuses::default()).await;
    proxy.sync().await;
    assert!(proxy.try_recv().await.is_none());

    // But the forcing one should.
    proxy.send(SubscribeToActorStatuses::forcing()).await;
    proxy.sync().await;

    check_seq(
        &mut proxy,
        &[
            (1, Normal, Some("on Start")),
            (3, Failed, Some("panic: oops")),
            (4, Normal, Some("on Start")),
            (5, Normal, Some("on Start")),
        ],
    )
    .await;

    // Fail some.
    proxy.send(Fail(1)).await;
    proxy.sync().await;

    check_seq(
        &mut proxy,
        &[
            (1, Normal, Some("on Fail")),
            (1, Failed, Some("panic: oops")),
        ],
    )
    .await;

    // Test another subscription.
    let mut subproxy = proxy.subproxy().await;
    subproxy.send(SubscribeToActorStatuses::default()).await;
    subproxy.sync().await;

    check_seq(
        &mut subproxy,
        &[
            (1, Failed, Some("panic: oops")),
            (3, Failed, Some("panic: oops")),
            (4, Normal, Some("on Start")),
            (5, Normal, Some("on Start")),
        ],
    )
    .await;

    // Stop some.
    proxy.send(Stop(4)).await;
    proxy.sync().await;
    subproxy.sync().await;

    check_seq(
        &mut proxy,
        &[(4, Normal, Some("on Stop")), (4, Terminated, None)],
    )
    .await;
    check_seq(
        &mut subproxy,
        &[(4, Normal, Some("on Stop")), (4, Terminated, None)],
    )
    .await;

    // Wait for restarting.
    tokio::time::sleep(Duration::from_secs(100)).await;
    proxy.sync().await;
    subproxy.sync().await;

    check_seq(
        &mut proxy,
        &[
            (1, Initializing, None),
            (1, Normal, None),
            (3, Initializing, None),
            (3, Normal, None),
        ],
    )
    .await;
    check_seq(
        &mut subproxy,
        &[
            (1, Initializing, None),
            (1, Normal, None),
            (3, Initializing, None),
            (3, Normal, None),
        ],
    )
    .await;
}
