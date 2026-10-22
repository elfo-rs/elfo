#![allow(missing_docs)]
#![cfg(feature = "test-util")]

use std::time::Duration;

use tracing::info;

use elfo::{
    _priv::do_start,
    Local, RestartParams, RestartPolicy, Topology,
    errors::RequestError,
    prelude::*,
    routers::{MapRouter, Outcome, Singleton},
    test::extract_request,
};
use elfo_core::{Addr, config::AnyConfig, messages::Ping};
use elfo_test::extract_message;

mod common;

#[message(ret = u64)]
struct TestRequest;

#[message]
struct NeverSent;

#[tokio::test]
async fn with_sender() {
    elfo::set_protocol!("with_sender");

    #[message(part)]
    #[derive(Copy, PartialEq, Eq, PartialOrd, Ord, Hash, derive_more::Display)]
    enum ActorKind {
        A,
        B,
        DropEnvelope,
        DropToken,
    }

    #[message(ret = ActorKind)]
    struct Query;

    #[message(ret = ActorKind)]
    struct WhoAmI;

    #[message(ret = ())]
    struct RunTest;

    async fn responder(mut ctx: Context<(), ActorKind>) {
        while let Some(envelope) = ctx.recv().await {
            if *ctx.key() == ActorKind::DropEnvelope && envelope.is::<Query>() {
                drop(envelope);
                continue;
            }
            msg!(match envelope {
                (Query, token) => match ctx.key() {
                    ActorKind::DropToken => drop(token),
                    key => ctx.respond(token, *key),
                },
                (WhoAmI, token) => ctx.respond(token, *ctx.key()),
                _ => unreachable!(),
            });
        }
    }

    async fn requester(mut ctx: Context) {
        let (RunTest, done) = extract_request(ctx.recv().await.unwrap());

        let (response, sender) = ctx.request(Query).resolve().with_sender().await;
        let key = response.unwrap();
        assert!(matches!(key, ActorKind::A | ActorKind::B));
        assert!(!sender.is_null());

        let (direct, direct_sender) = ctx.request_to(sender, WhoAmI).resolve().with_sender().await;
        assert_eq!(direct.unwrap(), key);
        assert_eq!(direct_sender, sender);

        let responses = ctx.request(Query).all().resolve().with_sender().await;
        let mut results = Vec::new();
        for (response, sender) in responses {
            assert!(!sender.is_null());
            let key = ctx.request_to(sender, WhoAmI).resolve().await.unwrap();
            results.push((key, response));
        }
        results.sort_by_key(|(key, _)| *key);
        assert!(matches!(
            results.as_slice(),
            [
                (ActorKind::A, Ok(ActorKind::A)),
                (ActorKind::B, Ok(ActorKind::B)),
                (ActorKind::DropEnvelope, Err(RequestError::Ignored)),
                (ActorKind::DropToken, Err(RequestError::Ignored)),
            ]
        ));

        ctx.respond(done, ());
    }

    common::setup_logger();
    let topology = Topology::empty();
    let configurers = topology.local("system.configurers").entrypoint();
    let requesters = topology.local("requesters");
    let requester_addr = requesters.addr();
    let responders = topology.local("responders");
    requesters.route_all_to(&responders);

    configurers.mount(elfo_configurer::fixture(&topology, AnyConfig::default()));
    requesters.mount(ActorGroup::new().exec(requester));
    responders.mount(
        ActorGroup::new()
            .router(MapRouter::new(|envelope| {
                msg!(match envelope {
                    Query => Outcome::Multicast(vec![
                        ActorKind::A,
                        ActorKind::B,
                        ActorKind::DropEnvelope,
                        ActorKind::DropToken,
                    ]),
                    _ => Outcome::Default,
                })
            }))
            .exec(responder),
    );

    do_start(topology, false, |ctx, _| async move {
        ctx.request_to(requester_addr, RunTest).resolve().await
    })
    .await
    .expect("cannot start")
    .expect("requester actor failed");
}

#[tokio::test]
async fn with_sender_on_failed_request() {
    elfo::set_protocol!("with_sender_on_failed_request");

    #[message(ret = ())]
    struct Query;

    #[message]
    struct Started(Local<Addr>);

    #[message(ret = ())]
    struct RunTest;

    async fn responder(ctx: Context<()>) {
        ctx.close();
        ctx.send(Started(ctx.group().into())).await.unwrap();
    }

    async fn requester(mut ctx: Context) {
        let (RunTest, done) = extract_request(ctx.recv().await.unwrap());

        let (response, sender) = ctx.request(Query).resolve().with_sender().await;
        assert!(matches!(response, Err(RequestError::Failed)));
        assert!(!sender.is_null());

        let msg = ctx.recv().await.unwrap();
        let addr = msg.sender();
        let Started(group) = extract_message(msg);
        assert_eq!(sender, addr);

        // A request to the terminated actor cannot be delivered, so the
        // responder is unknown even though the destination is.
        let (response, sender) = ctx
            .request_to(addr, Ping::default())
            .resolve()
            .with_sender()
            .await;
        assert!(matches!(response, Err(RequestError::Failed)));
        assert!(sender.is_null());

        // No recipients, no address.
        let (response, sender) = ctx.request(Ping::default()).resolve().with_sender().await;
        assert!(matches!(response, Err(RequestError::Failed)));
        assert_eq!(sender, Addr::NULL);

        // No recipients in the group, no address either.
        let (response, sender) = ctx
            .request_to(*group, Ping::default())
            .resolve()
            .with_sender()
            .await;
        assert!(matches!(response, Err(RequestError::Failed)));
        assert!(sender.is_null());

        ctx.respond(done, ());
    }

    common::setup_logger();
    let topology = Topology::empty();
    let configurers = topology.local("system.configurers").entrypoint();
    let requesters = topology.local("requesters");
    let requester_addr = requesters.addr();
    let responders = topology.local("responders");
    requesters.route_all_to(&responders);
    responders.route_all_to(&requesters);

    configurers.mount(elfo_configurer::fixture(&topology, AnyConfig::default()));
    requesters.mount(ActorGroup::new().exec(requester));
    responders.mount(
        ActorGroup::new()
            .router(MapRouter::new(|envelope| {
                msg!(match envelope {
                    Query => Outcome::Unicast(Singleton),
                    _ => Outcome::Default,
                })
            }))
            .exec(responder),
    );

    do_start(topology, false, |ctx, _| async move {
        ctx.request_to(requester_addr, RunTest).resolve().await
    })
    .await
    .expect("cannot start")
    .expect("requester actor failed");
}

#[tokio::test]
async fn stealing() {
    common::setup_logger();

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

    configurers.mount(elfo::batteries::configurer::fixture(
        &topology,
        AnyConfig::default(),
    ));
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
    common::setup_logger();

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
            assert_eq!(responses.len(), 7, "{responses:?}");
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
