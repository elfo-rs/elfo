#![cfg(feature = "test-util")]

use std::time::Duration;

use elfo::{
    messages::{ActorStatusReport, SubscribeToActorStatuses},
    prelude::*,
    routers::{MapRouter, Outcome},
    test::Proxy,
    ActorStatus, ActorStatusKind, Envelope,
};

#[message]
struct Start(u32);

#[message]
struct Stop(u32);

#[message]
struct Fail(u32);

async fn run_group() -> Proxy {
    let schema = ActorGroup::new()
        .router(MapRouter::new(|envelope| {
            msg!(match envelope {
                Start(n) | Stop(n) | Fail(n) => Outcome::Unicast(*n),
                _ => Outcome::Default,
            })
        }))
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

    elfo::test::proxy(schema, elfo::config::AnyConfig::default()).await
}

#[track_caller]
fn check(envelope: &Envelope, key: u32, kind: ActorStatusKind, details: Option<&str>) {
    msg!(match envelope {
        ActorStatusReport { meta, status, .. } => {
            assert_eq!(meta.group, "subject");
            assert_eq!(meta.key, key.to_string());
            assert_eq!(status.kind(), kind);
            assert_eq!(status.details(), details);
        }
        _ => unreachable!(),
    })
}

#[tokio::test]
async fn it_works() {
    tokio::time::pause();

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

    // Firstly, we should receive statuses of unstopped actors.
    check(&proxy.recv().await, 3, Failed, Some("panic: oops"));
    check(&proxy.recv().await, 1, Normal, Some("on Start"));
    check(&proxy.recv().await, 4, Normal, Some("on Start"));
    // There is no status for `2` because it's terminated.

    // Start another one.
    proxy.send(Start(5)).await;
    check(&proxy.recv().await, 5, Initializing, None);
    check(&proxy.recv().await, 5, Normal, None);
    check(&proxy.recv().await, 5, Normal, Some("on Start"));

    // Fail some.
    proxy.send(Fail(1)).await;
    check(&proxy.recv().await, 1, Normal, Some("on Fail"));
    check(&proxy.recv().await, 1, Failed, Some("panic: oops"));

    // Test another subscription.
    let mut subproxy = proxy.subproxy().await;
    subproxy.send(SubscribeToActorStatuses::default()).await;
    check(&subproxy.recv().await, 3, Failed, Some("panic: oops"));
    check(&subproxy.recv().await, 5, Normal, Some("on Start"));
    check(&subproxy.recv().await, 1, Failed, Some("panic: oops"));
    check(&subproxy.recv().await, 4, Normal, Some("on Start"));

    // Stop some.
    proxy.send(Stop(4)).await;
    check(&proxy.recv().await, 4, Normal, Some("on Stop"));
    check(&proxy.recv().await, 4, Terminated, None);
    check(&subproxy.recv().await, 4, Normal, Some("on Stop"));
    check(&subproxy.recv().await, 4, Terminated, None);

    proxy.sync().await;
    assert!(proxy.try_recv().is_none());
    subproxy.sync().await;
    assert!(subproxy.try_recv().is_none());

    // Wait for restarting.
    tokio::time::sleep(Duration::from_secs(100)).await;

    check(&proxy.recv().await, 3, Initializing, None);
    check(&proxy.recv().await, 3, Normal, None);
    check(&proxy.recv().await, 1, Initializing, None);
    check(&proxy.recv().await, 1, Normal, None);
    check(&subproxy.recv().await, 3, Initializing, None);
    check(&subproxy.recv().await, 3, Normal, None);
    check(&subproxy.recv().await, 1, Initializing, None);
    check(&subproxy.recv().await, 1, Normal, None);

    proxy.sync().await;
    assert!(proxy.try_recv().is_none());
    subproxy.sync().await;
    assert!(subproxy.try_recv().is_none());
}
