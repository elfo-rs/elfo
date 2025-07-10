#![allow(missing_docs)]
#![cfg(feature = "test-util")]
#![allow(clippy::never_loop)]

use elfo::{
    messages::{Terminate, TerminateReason},
    prelude::*,
    TerminationPolicy,
};

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
                        ctx.send(TerminateResponse(
                            msg.reason.expect("should receive reason"),
                        ))
                        .await
                        .unwrap();
                        break;
                    }
                    _ => unreachable!(),
                });
            }
        });

    let mut proxy = elfo::test::proxy(blueprint, elfo::config::AnyConfig::default()).await;

    let reason = TerminateReason::custom("custom reason");
    proxy.send(Terminate::with_reason(reason.clone())).await;
    assert_msg_eq!(proxy.recv().await, TerminateResponse(reason));
    assert!(proxy.try_recv().await.is_none());
}

#[message]
#[derive(PartialEq)]
struct TerminateResponse(TerminateReason);
