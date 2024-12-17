#![allow(missing_docs)]
#![cfg(feature = "test-util")]
#![allow(clippy::never_loop)]

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use toml::toml;

use elfo::{
    prelude::*,
    routers::{MapRouter, Outcome, Singleton},
    RestartParams, RestartPolicy,
};

#[tokio::test]
async fn actor_restarts_explicitly() {
    #[message]
    struct Terminate;

    #[message]
    struct Terminated;

    let blueprint = ActorGroup::new().exec(move |mut ctx| async move {
        while let Some(envelope) = ctx.recv().await {
            msg!(match envelope {
                Terminate { .. } => break,
                _ => unreachable!(),
            });
        }
        ctx.send(Terminated).await.unwrap();
    });

    let mut proxy = elfo::test::proxy(blueprint, elfo::config::AnyConfig::default()).await;

    for _ in 1..5 {
        proxy.send(Terminate).await;
        assert_msg!(proxy.recv().await, Terminated);
    }
}

#[tokio::test(start_paused = true)]
async fn actor_restarts_with_timeout_after_failures() {
    #[message]
    struct HealthCheck;

    #[message]
    struct Spawn;

    let blueprint = ActorGroup::new()
        .router(MapRouter::new(|e| {
            msg!(match e {
                Spawn => Outcome::Unicast(Singleton),
                // HealthCheck should not spawn the actor.
                HealthCheck => Outcome::GentleUnicast(Singleton),
                _ => Outcome::Discard,
            })
        }))
        .restart_policy(RestartPolicy::on_failure(RestartParams::new(
            Duration::from_secs(5),
            Duration::from_secs(30),
        )))
        .exec(move |mut ctx| async move {
            while let Some(envelope) = ctx.recv().await {
                msg!(match envelope {
                    HealthCheck | Spawn => panic!("boom!"),
                    _ => unreachable!(),
                });
            }
        });

    let proxy = elfo::test::proxy(blueprint, elfo::config::AnyConfig::default()).await;
    proxy.send(Spawn).await;

    for i in 1..5 {
        proxy.send(HealthCheck).await;
        let delay = Duration::from_millis(((5000f64 * 2.0f64.powi(i)) as u64).min(30000));
        // https://github.com/tokio-rs/tokio/issues/3985
        tokio::time::sleep(delay).await;
    }
}

#[tokio::test(start_paused = true)]
async fn restart_policy_overriding() {
    #[message]
    struct Started;

    let config = toml! {
        [system.restart_policy]
        when = "Always"
        min_backoff = "10s"
        max_backoff = "30s"
    };

    // The config overrides the default group policy. The default policy here is set
    // to RestartPolicy::never().
    let blueprint = ActorGroup::new().exec(move |ctx| async move {
        let _ = ctx.send(Started).await;
    });

    let mut proxy = elfo::test::proxy(blueprint, config.clone()).await;
    assert_msg!(proxy.recv().await, Started);
    assert_msg!(proxy.recv().await, Started);

    // The actor overrides config policy.
    let blueprint = ActorGroup::new().exec(move |ctx| async move {
        ctx.set_restart_policy(RestartPolicy::never());
        let _ = ctx.send(Started).await;
    });
    let mut proxy = elfo::test::proxy(blueprint, config.clone()).await;
    assert_msg!(proxy.recv().await, Started);
    proxy.sync().await;
    assert!(proxy.try_recv().await.is_none());

    // if the actor resets the restart policy with `None`, the restart policy is
    // reverted back to the group policy.
    let blueprint = ActorGroup::new().exec(move |ctx| async move {
        ctx.set_restart_policy(RestartPolicy::never());
        ctx.set_restart_policy(None);
        let _ = ctx.send(Started).await;
    });

    let mut proxy = elfo::test::proxy(blueprint, config).await;
    assert_msg!(proxy.recv().await, Started);
    assert_msg!(proxy.recv().await, Started);
}

#[message(ret = ())]
#[derive(Default)]
struct GuardedMessage(Arc<Mutex<bool>>);

impl Drop for GuardedMessage {
    fn drop(&mut self) {
        *self.0.lock().unwrap() = true;
    }
}

// See #68.
#[tokio::test(start_paused = true)]
async fn mailbox_must_be_dropped() {
    let blueprint = ActorGroup::new().exec(|_ctx| async {
        tokio::time::sleep(Duration::from_secs(1)).await;
        anyhow::bail!("boom!");
    });
    let proxy = elfo::test::proxy(blueprint, elfo::config::AnyConfig::default()).await;

    let msg = GuardedMessage::default();
    let flag = msg.0.clone();
    proxy.send(msg).await;

    assert!(!*flag.lock().unwrap());

    // On failure, the mailbox must be dropped.
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert!(*flag.lock().unwrap());
}
