#![allow(missing_docs)]
#![cfg(feature = "test-util")]
#![allow(clippy::never_loop)]
use std::{pin::pin, time::Duration};

use elfo::{
    TerminationPolicy,
    messages::{Ping, Terminate, TerminateReason},
    prelude::*,
};
use futures::poll;
use tokio::time::sleep;

#[message]
#[derive(PartialEq)]
struct BeforeExit;

#[tokio::test]
async fn it_terminates_closing_policy() {
    let blueprint = ActorGroup::new().exec(move |mut ctx| async move {
        while let Some(envelope) = ctx.recv().await {
            msg!(match envelope {
                _ => unreachable!(),
            });
        }

        ctx.send(BeforeExit).await.unwrap();
    });

    let mut proxy = elfo::test::proxy(blueprint, elfo::config::AnyConfig::default()).await;

    proxy.send(Terminate::default()).await;
    proxy.finished().await;
    assert_msg_eq!(proxy.recv().await, BeforeExit);
    proxy.sync().await;
}

#[tokio::test]
async fn it_terminates_manually_policy() {
    let blueprint = ActorGroup::new()
        .termination_policy(TerminationPolicy::manually())
        .exec(move |mut ctx| async move {
            while let Some(envelope) = ctx.recv().await {
                msg!(match envelope {
                    Terminate => {
                        ctx.send(BeforeExit).await.unwrap();
                        return;
                    }
                    _ => unreachable!(),
                });
            }
        });

    let mut proxy = elfo::test::proxy(blueprint, elfo::config::AnyConfig::default()).await;

    proxy.send(Terminate::default()).await;
    proxy.finished().await;
    assert_msg_eq!(proxy.recv().await, BeforeExit);
    proxy.sync().await;
}

#[tokio::test]
async fn it_terminates_manually_policy_via_closing_terminate() {
    let blueprint = ActorGroup::new()
        .termination_policy(TerminationPolicy::manually())
        .exec(move |mut ctx| async move {
            while let Some(envelope) = ctx.recv().await {
                msg!(match envelope {
                    Terminate => {
                        ctx.send(BeforeExit).await.unwrap();
                    }
                    _ => unreachable!(),
                });
            }

            ctx.send(BeforeExit).await.unwrap();
        });

    let mut proxy = elfo::test::proxy(blueprint, elfo::config::AnyConfig::default()).await;

    proxy.send(Terminate::default()).await;
    assert_msg_eq!(proxy.recv().await, BeforeExit);
    assert!(proxy.try_recv().await.is_none());

    proxy.send(Terminate::closing()).await;
    assert_msg_eq!(proxy.recv().await, BeforeExit);
    proxy.finished().await;
    proxy.sync().await;
}

#[tokio::test]
async fn terminate_with_reason() {
    let blueprint = ActorGroup::new()
        .termination_policy(TerminationPolicy::manually())
        .exec(move |mut ctx| async move {
            while let Some(envelope) = ctx.recv().await {
                msg!(match envelope {
                    msg @ Terminate => {
                        ctx.send(TerminateResponse(msg.reason)).await.unwrap();
                        break;
                    }
                });
            }
        });

    let mut proxy = elfo::test::proxy(blueprint, elfo::config::AnyConfig::default()).await;

    let reason = TerminateReason::custom("custom reason");
    proxy
        .send(Terminate::default().with_reason(reason.clone()))
        .await;
    assert_msg_eq!(proxy.recv().await, TerminateResponse(reason));
    assert!(proxy.try_recv().await.is_none());
}

// A `Context` leaked into a detached task outlives the actor's `exec` future.
// While the context is alive, the mailbox cannot be drained, so pending
// requests stay unresolved past actor termination.
#[tokio::test]
async fn context_outlives_actor() {
    tokio::time::pause();

    let blueprint = ActorGroup::new()
        .termination_policy(TerminationPolicy::manually())
        .exec(move |ctx| async move {
            // `exec` returns at t=1s, but the spawned task keeps the context
            // alive until t=6s.
            sleep(std::time::Duration::from_secs(1)).await;
            // Steal context and extend its live
            tokio::spawn(async move {
                let _ctx = ctx;
                sleep(std::time::Duration::from_secs(5)).await;
            });
        });
    let mut proxy = elfo::test::proxy(blueprint, elfo::config::AnyConfig::default()).await;

    // Issue a request that nobody will read from the mailbox.
    let requester = proxy.subproxy().await;
    let mut fut = pin!(requester.request_fallible(Ping::default()));
    assert!(poll!(&mut fut).is_pending());

    // t=2s: `exec` has returned, the actor is in a finished state.
    tokio::time::advance(Duration::from_secs(2)).await;
    proxy.sync().await;

    // The mailbox is closed for new producers.
    assert!(proxy.try_send(Ping::default()).is_err());
    // But the in-flight request still hangs: the leaked context blocks
    // mailbox draining, so pending requests are not resolved yet.
    assert!(poll!(&mut fut).is_pending());

    // t=8s: the spawned task ends and drops the context. The mailbox is
    // finally drained, and pending requests resolve with `RequestError`.
    tokio::time::advance(Duration::from_secs(6)).await;
    proxy.sync().await;
    assert!(poll!(&mut fut).is_ready());
}

#[message]
#[derive(PartialEq)]
struct TerminateResponse(TerminateReason);
