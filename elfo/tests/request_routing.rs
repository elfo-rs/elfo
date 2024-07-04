#![cfg(feature = "test-util")]

use std::time::Duration;

use tracing::info;

use elfo::{
    prelude::*,
    routers::{MapRouter, Outcome},
    RestartParams, RestartPolicy, Topology,
    _priv::do_start,
};
use elfo_core::config::AnyConfig;

#[message(ret = u64)]
struct TestRequest;

#[message]
struct NeverSent;

fn setup_logger() {
    let _ = tracing_subscriber::fmt()
        .with_target(false)
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_test_writer()
        .try_init();
}

#[tokio::test]
async fn stealing() {
    setup_logger();

    let requester_blueprint = ActorGroup::new().exec(move |mut ctx| async move {
        info!("sent request");
        ctx.request(TestRequest).resolve().await.unwrap();

        let envelope = ctx.recv().await.unwrap();
        msg!(match envelope {
            (TestRequest, token) => ctx.respond(token, 42),
            _ => unreachable!(),
        });
    });

    let responder_blueprint = ActorGroup::new().exec(move |mut ctx| async move {
        while let Some(envelope) = ctx.recv().await {
            msg!(match envelope {
                (TestRequest, token) => {
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

    let topology = Topology::empty();
    let configurers = topology.local("system.configurers").entrypoint();
    let requester = topology.local("requester");
    let requester_addr = requester.addr();
    let responder = topology.local("responder");
    let thief = topology.local("thief");

    requester.route_all_to(&thief);
    requester.route_to(&responder, |e| {
        msg!(match e {
            TestRequest => true,
            _ => false,
        })
    });

    configurers.mount(elfo_configurer::fixture(&topology, AnyConfig::default()));
    requester.mount(requester_blueprint);
    responder.mount(responder_blueprint);
    thief.mount(thief_blueprint);

    do_start(topology, false, |ctx, _| async move {
        ctx.request_to(requester_addr, TestRequest).resolve().await
    })
    .await
    .expect("cannot start")
    .expect("requester actor failed");
}

#[tokio::test]
async fn multiple_failures() {
    setup_logger();

    fn success_blueprint(id: u64) -> Blueprint {
        ActorGroup::new().exec(move |mut ctx| async move {
            while let Some(envelope) = ctx.recv().await {
                msg!(match envelope {
                    (TestRequest, token) => ctx.respond(token, id),
                    _ => unreachable!(),
                })
            }
        })
    }

    fn failure_blueprint() -> Blueprint {
        ActorGroup::new()
            .restart_policy(RestartPolicy::on_failure(RestartParams::new(
                Duration::from_secs(1000),
                Duration::from_secs(1000),
            )))
            .exec(|_ctx| async move {
                panic!("failure");
            })
    }

    let topology = Topology::empty();
    let configurers = topology.local("system.configurers").entrypoint();
    let requester = topology.local("requester");
    let requester_addr = requester.addr();
    let responder_1_fail = topology.local("responder_1_fail");
    let responder_2_fail = topology.local("responder_2_fail");
    let responder_3_fail = topology.local("responder_3_fail");
    let responder_4_succ = topology.local("responder_4_succ");
    let responder_5_fail = topology.local("responder_5_fail");
    let responder_6_succ = topology.local("responder_6_succ");
    let responder_7_fail = topology.local("responder_7_fail");

    requester.route_all_to(&responder_1_fail);
    requester.route_all_to(&responder_2_fail);
    requester.route_all_to(&responder_3_fail);
    requester.route_all_to(&responder_4_succ);
    requester.route_all_to(&responder_5_fail);
    requester.route_all_to(&responder_6_succ);
    requester.route_all_to(&responder_7_fail);

    let requester_blueprint = ActorGroup::new().exec(move |mut ctx| async move {
        for i in 0..10 {
            info!("iter #{i}");
            info!("sent any-request");
            let response = ctx.request(TestRequest).resolve().await.unwrap();
            assert!(response == 4 || response == 6);

            info!("sent all-request");
            let responses = ctx.request(TestRequest).all().resolve().await;
            assert_eq!(responses.len(), 7, "{:?}", responses);
        }

        let envelope = ctx.recv().await.unwrap();
        msg!(match envelope {
            (TestRequest, token) => ctx.respond(token, 42),
            _ => unreachable!(),
        });
    });

    configurers.mount(elfo_configurer::fixture(&topology, AnyConfig::default()));
    requester.mount(requester_blueprint);
    responder_1_fail.mount(failure_blueprint());
    responder_2_fail.mount(failure_blueprint());
    responder_3_fail.mount(failure_blueprint());
    responder_4_succ.mount(success_blueprint(4));
    responder_5_fail.mount(failure_blueprint());
    responder_6_succ.mount(success_blueprint(6));
    responder_7_fail.mount(failure_blueprint());

    do_start(topology, false, |ctx, _| async move {
        ctx.request_to(requester_addr, TestRequest).resolve().await
    })
    .await
    .expect("cannot start")
    .expect("requester actor failed");
}
