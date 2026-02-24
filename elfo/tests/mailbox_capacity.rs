#![allow(missing_docs)]
#![cfg(feature = "test-util")]

use std::time::Duration;

use serde::Deserialize;
use toml::toml;

use elfo::{
    config::AnyConfig,
    messages::{Ping, Terminate, UpdateConfig},
    prelude::*,
};

#[message]
struct Dummy;

#[message(ret = ())]
struct Freeze;

#[message]
struct SendDummy;

#[message]
struct SetCapacity(Option<usize>);

fn testee() -> Blueprint {
    ActorGroup::new().exec(move |mut ctx| async move {
        while let Some(envelope) = ctx.recv().await {
            let sender = envelope.sender();
            msg!(match envelope {
                Dummy => {}
                SendDummy => {
                    ctx.send_to(sender, Dummy).await.unwrap();
                }
                (Freeze, token) => {
                    ctx.respond(token, ());
                    tokio::time::sleep(Duration::from_secs(60)).await
                }
                SetCapacity(capacity) => ctx.set_mailbox_capacity(capacity),
            });
        }
    })
}

fn testee_config(capacity: usize) -> AnyConfig {
    AnyConfig::deserialize(toml! {
        system.mailbox.capacity = capacity
    })
    .unwrap()
}

#[tokio::test(start_paused = true)]
async fn unbounded_sends() {
    let mut proxy = elfo::test::proxy(testee(), testee_config(1)).await;

    // 1. Unbounded sends occupy capacity when it's possible.

    proxy.request(Freeze).await;

    proxy.unbounded_send(Dummy);
    assert!(
        proxy.try_send(Dummy).is_err(),
        "the place is taken by unbounded_send above"
    );

    proxy.request(Ping::default()).await;

    // 2. When there's no left capacity - unbounded sends work
    // anyway.

    proxy.request(Freeze).await;
    proxy
        .try_send(Dummy)
        .expect("must occupy 1 slot successfully");
    proxy.unbounded_send(SendDummy);

    // Must send us dummy back, since mailbox is [Dummy, SendDummy]
    // even when it's size is configured to be 1.
    let Dummy = elfo::test::extract_message(proxy.recv().await);
}

#[tokio::test(start_paused = true)]
#[should_panic(expected = "cannot send Dummy (mailbox closed) unboundedly")]
async fn unbounded_send_panic() {
    let proxy = elfo::test::proxy(testee(), testee_config(1)).await;
    proxy.send(Terminate::closing()).await;

    proxy.unbounded_send(Dummy);
}

#[tokio::test(start_paused = true)]
#[should_panic(expected = "cannot send Dummy (mailbox closed) unboundedly")]
async fn unbounded_send_to_panic() {
    let mut proxy = elfo::test::proxy(testee(), testee_config(1)).await;
    proxy.send(SendDummy).await;
    let testee_addr = proxy.recv().await.sender();

    proxy.send(Terminate::closing()).await;
    proxy.finished().await;
    proxy.unbounded_send_to(testee_addr, Dummy);
}

#[tokio::test(start_paused = true)]
async fn config() {
    let proxy = elfo::test::proxy(testee(), testee_config(0)).await;

    for capacity in [1, 10, 100, 1000, 500, 50, 5, 15, 150] {
        proxy.send(UpdateConfig::new(testee_config(capacity))).await;
        proxy.request(Freeze).await;

        for i in 1..=capacity {
            assert!(proxy.try_send(Dummy).is_ok(), "shoud pass [{i}/{capacity}]");
        }
        assert!(proxy.try_send(Dummy).is_err(), "should reject [{capacity}]");

        // Ensure that all sent messages are handled.
        proxy.request(Ping::default()).await;
    }
}

#[tokio::test(start_paused = true)]
async fn explicit_override() {
    let configured_capacity = 42;
    let proxy = elfo::test::proxy(testee(), testee_config(configured_capacity)).await;

    for capacity in [1, 10, 100, 1000, 500, 50, 5, 15, 150] {
        proxy.send(SetCapacity(Some(capacity))).await;
        proxy.request(Freeze).await;

        for i in 1..=capacity {
            assert!(proxy.try_send(Dummy).is_ok(), "shoud pass [{i}/{capacity}]");
        }
        assert!(proxy.try_send(Dummy).is_err(), "should reject [{capacity}]");

        // Ensure that all sent messages are handled.
        proxy.request(Ping::default()).await;
    }

    // Reset to the configured value.
    proxy.send(SetCapacity(None)).await;
    proxy.request(Freeze).await;

    for i in 1..=configured_capacity {
        assert!(proxy.try_send(Dummy).is_ok(), "shoud pass [{i}/configured]");
    }
    assert!(proxy.try_send(Dummy).is_err(), "should reject [configured]");
}
