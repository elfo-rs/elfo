#![allow(missing_docs)]
#![cfg(feature = "test-util")]

use std::time::Duration;

use serde::Deserialize;
use toml::toml;

use elfo::{
    config::AnyConfig,
    messages::{Ping, UpdateConfig},
    prelude::*,
};

#[message]
struct Dummy;

#[message(ret = ())]
struct Freeze;

#[message]
struct SetCapacity(Option<usize>);

fn testee() -> Blueprint {
    ActorGroup::new().exec(move |mut ctx| async move {
        while let Some(envelope) = ctx.recv().await {
            msg!(match envelope {
                Dummy => {}
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
