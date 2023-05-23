#![cfg(feature = "test-util")]

use elfo::{
    config::AnyConfig,
    errors::RequestError,
    messages::{ConfigRejected, ValidateConfig},
    prelude::*,
    routers::{MapRouter, Outcome, Singleton},
};
use tracing::info;

#[message]
struct StartSingleton;

#[tokio::test]
async fn singleton_actor_with_default_validate_config() {
    let blueprint = ActorGroup::new().exec(move |mut ctx| async move {
        while let Some(envelope) = ctx.recv().await {
            msg!(match envelope {
                StartSingleton => continue,
                (ValidateConfig { .. }, token) => {
                    let error: ConfigRejected =
                        String::from("did not expect ValidateConfig message").into();
                    ctx.respond(token, Err(error));
                }
                _ => unreachable!(),
            });
        }
    });

    let mut proxy = elfo::test::proxy(blueprint, AnyConfig::default()).await;
    info!("proxy started");

    proxy.send(StartSingleton).await;
    proxy.sync().await;
    info!("actor started");

    let result = proxy
        .try_request(ValidateConfig::new(AnyConfig::default()))
        .await;
    assert!(matches!(result, Err(RequestError::Closed(..))));
}

#[tokio::test]
async fn singleton_actor_with_custom_validate_config() {
    let blueprint = ActorGroup::new()
        .router(MapRouter::new(|e| {
            msg!(match e {
                StartSingleton => Outcome::Unicast(Singleton),
                ValidateConfig => Outcome::GentleUnicast(Singleton),
                _ => Outcome::Default,
            })
        }))
        .exec(move |mut ctx| async move {
            while let Some(envelope) = ctx.recv().await {
                msg!(match envelope {
                    StartSingleton => continue,
                    (ValidateConfig { .. }, token) => {
                        ctx.respond(token, Ok(()));
                    }
                    _ => unreachable!(),
                });
            }
        });

    let mut proxy = elfo::test::proxy(blueprint, AnyConfig::default()).await;
    info!("proxy started");

    proxy.send(StartSingleton).await;
    proxy.sync().await;
    info!("actor started");

    proxy
        .request(ValidateConfig::new(AnyConfig::default()))
        .await
        .unwrap();
}

#[message]
struct StartGroupMember(u32);

#[tokio::test]
async fn actor_group_with_default_validate_config() {
    let blueprint = ActorGroup::new()
        .router(MapRouter::new(|e| {
            msg!(match e {
                StartGroupMember(no) => Outcome::Unicast(*no),
                _ => Outcome::Default,
            })
        }))
        .exec(move |mut ctx| async move {
            while let Some(envelope) = ctx.recv().await {
                msg!(match envelope {
                    StartGroupMember(..) => continue,
                    (ValidateConfig { .. }, token) => {
                        let error: ConfigRejected =
                            String::from("did not expect ValidateConfig message").into();
                        ctx.respond(token, Err(error));
                    }
                    _ => unreachable!(),
                });
            }
        });

    let mut proxy = elfo::test::proxy(blueprint, AnyConfig::default()).await;
    info!("proxy started");

    proxy.send(StartGroupMember(0)).await;
    proxy.send(StartGroupMember(1)).await;
    proxy.send(StartGroupMember(2)).await;
    proxy.sync().await;
    info!("actors started");

    let result = proxy
        .try_request(ValidateConfig::new(AnyConfig::default()))
        .await;
    assert!(matches!(result, Err(RequestError::Closed(..))));
}

#[tokio::test]
async fn actor_group_with_custom_validate_config() {
    let blueprint = ActorGroup::new()
        .router(MapRouter::new(|e| {
            msg!(match e {
                StartGroupMember(no) => Outcome::Unicast(*no),
                ValidateConfig => Outcome::Broadcast,
                _ => Outcome::Default,
            })
        }))
        .exec(move |mut ctx| async move {
            while let Some(envelope) = ctx.recv().await {
                msg!(match envelope {
                    StartGroupMember(..) => continue,
                    (ValidateConfig { .. }, token) => {
                        ctx.respond(token, Ok(()));
                    }
                    _ => unreachable!(),
                });
            }
        });

    let mut proxy = elfo::test::proxy(blueprint, AnyConfig::default()).await;
    info!("proxy started");

    proxy.send(StartGroupMember(0)).await;
    proxy.send(StartGroupMember(1)).await;
    proxy.send(StartGroupMember(2)).await;
    proxy.sync().await;
    info!("actors started");

    proxy
        .request(ValidateConfig::new(AnyConfig::default()))
        .await
        .unwrap();
}
