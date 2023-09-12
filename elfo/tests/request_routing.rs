#![cfg(feature = "test-util")]

use std::sync::Arc;

use elfo::{
    prelude::*,
    routers::{MapRouter, Outcome},
    Topology,
    _priv::do_start,
};
use serde::Deserialize;
use serde_value::Value;
use tracing::info;

#[message(ret = u64)]
struct TestRequest;

#[message]
struct NeverSent;

#[tokio::test]
async fn test_stealing_request_routing() {
    let (tx, rx) = futures_intrusive::channel::shared::oneshot_channel();
    let tx = Arc::new(tx);

    let requester_blueprint = ActorGroup::new().exec(move |ctx| {
        let tx = tx.clone();

        async move {
            info!("sent request");
            match ctx.request(TestRequest).resolve().await {
                Ok(_) => info!("got response"),
                Err(_) => {
                    tx.send(false).unwrap();
                    return;
                }
            }
            tx.send(true).unwrap();
        }
    });

    let responder_blueprint = ActorGroup::new().exec(move |mut ctx| async move {
        while let Some(envelope) = ctx.recv().await {
            msg!(match envelope {
                (_req @ TestRequest, token) => {
                    ctx.respond(token, 42);
                    info!("replied to request");
                }
                _ => panic!("responder got unexpected message"),
            });
        }
    });

    let thief_blueprint = ActorGroup::new()
        .router(MapRouter::new(|envelope| {
            msg!(match envelope {
                NeverSent => Outcome::Unicast(0),
                _ => Outcome::Default,
            })
        }))
        .exec(move |_ctx| async move {
            panic!("thief should not be started");
        });

    let _ = tracing_subscriber::fmt()
        .with_target(false)
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_test_writer()
        .try_init();

    let config = toml::toml! {
        [requester]
        [responder]
        [thief]
    };
    let config = Value::deserialize(config).expect("invalid config");

    let topology = Topology::empty();
    let configurers = topology.local("system.configurers").entrypoint();
    let requester = topology.local("requester");
    let responder = topology.local("responder");
    let thief = topology.local("thief");

    requester.route_all_to(&thief);
    requester.route_to(&responder, |e| {
        msg!(match e {
            TestRequest => true,
            _ => false,
        })
    });

    configurers.mount(elfo_configurer::fixture(&topology, config));
    requester.mount(requester_blueprint);
    responder.mount(responder_blueprint);
    thief.mount(thief_blueprint);

    do_start(topology, false, |_, _| futures::future::ready(()))
        .await
        .expect("cannot start");

    let success = rx.receive().await.unwrap();
    assert!(success);
}
