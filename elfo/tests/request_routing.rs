#![allow(missing_docs)]
#![cfg(feature = "test-util")]

use std::{pin::pin, time::Duration};

use futures::poll;
use tracing::info;

use elfo::{
    _priv::do_start,
    RestartParams, RestartPolicy, Topology,
    errors::RequestError,
    prelude::*,
    routers::{MapRouter, Outcome},
    test::extract_request,
};
use elfo_core::{Addr, config::AnyConfig, messages::Ping};

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

    #[message(part)]
    #[derive(Copy, PartialEq, Eq, PartialOrd, Ord, Hash, derive_more::Display)]
    enum ActorKind {
        ClosedMailbox,
        Ok,
        Full,
    }

    #[message(ret = ())]
    struct Spawn(ActorKind);

    #[message(ret = ActorKind)]
    struct Query;

    #[message(ret = ())]
    struct ConfirmMailboxClose;

    #[message(ret = ())]
    struct ConfirmTermination;

    #[message(ret = ())]
    struct ConfirmResume;

    #[message(ret = ())]
    struct RunTest;

    async fn responder(mut ctx: Context<(), ActorKind>) {
        match ctx.key() {
            ActorKind::ClosedMailbox => {
                // Spawn occupies the only slot while delivery is being tested.
                ctx.set_mailbox_capacity(Some(1));
                ctx.request(ConfirmMailboxClose).resolve().await.unwrap();
                ctx.close();
                ctx.request(ConfirmTermination).resolve().await.unwrap();
                return;
            }
            ActorKind::Full => {
                // Spawn occupies the only slot until reading is allowed.
                ctx.set_mailbox_capacity(Some(1));
                ctx.request(ConfirmResume).resolve().await.unwrap();
            }
            ActorKind::Ok => {}
        }

        while let Some(envelope) = ctx.recv().await {
            msg!(match envelope {
                (Spawn(_), token) => ctx.respond(token, ()),
                (Query, token) => ctx.respond(token, *ctx.key()),
                _ => unreachable!(),
            });
        }
    }

    async fn requester(mut ctx: Context, responders_addr: Addr) {
        let (RunTest, done) = extract_request(ctx.recv().await.unwrap());

        let pruned = ctx.pruned();
        let mut queued = pin!(
            pruned
                .request(Spawn(ActorKind::ClosedMailbox))
                .resolve()
                .with_sender()
        );
        assert!(poll!(queued.as_mut()).is_pending());

        let msg = ctx.recv().await.unwrap();
        let closed_addr = msg.sender();
        let (ConfirmMailboxClose, close) = extract_request(msg);

        // Closing the mailbox fails sends that are waiting for capacity.
        let mut routed = pin!(pruned.request(Ping::default()).resolve().with_sender());
        assert!(poll!(routed.as_mut()).is_pending());
        let mut group = pin!(
            pruned
                .request_to(responders_addr, Ping::default())
                .resolve()
                .with_sender()
        );
        assert!(poll!(group.as_mut()).is_pending());
        let mut direct = pin!(
            pruned
                .request_to(closed_addr, Ping::default())
                .resolve()
                .with_sender()
        );
        assert!(poll!(direct.as_mut()).is_pending());
        ctx.respond(close, ());

        let (routed, group, direct) = futures::join!(routed, group, direct);
        for (response, sender) in [routed, group, direct] {
            assert!(matches!(response, Err(RequestError::Failed)));
            assert_eq!(sender, closed_addr);
        }

        let (ConfirmTermination, terminate) = extract_request(ctx.recv().await.unwrap());
        assert!(poll!(queued.as_mut()).is_pending());

        // A direct request to a closed mailbox.
        let (response, sender) = ctx
            .request_to(closed_addr, Ping::default())
            .resolve()
            .with_sender()
            .await;
        assert!(matches!(response, Err(RequestError::Failed)));
        assert_eq!(sender, closed_addr);

        // The same via routing.
        let (response, sender) = ctx.request(Ping::default()).resolve().with_sender().await;
        assert!(matches!(response, Err(RequestError::Failed)));
        assert_eq!(sender, closed_addr);

        // Direct via group addr.
        let (response, sender) = ctx
            .request_to(responders_addr, Ping::default())
            .resolve()
            .with_sender()
            .await;
        assert!(matches!(response, Err(RequestError::Failed)));
        assert_eq!(sender, closed_addr);

        let (response, ok_addr) = ctx
            .request(Spawn(ActorKind::Ok))
            .resolve()
            .with_sender()
            .await;
        response.unwrap();

        ctx.send(Spawn(ActorKind::Full)).await.unwrap();
        let msg = ctx.recv().await.unwrap();
        let full_addr = msg.sender();
        let (ConfirmResume, resume) = extract_request(msg);

        // All three delivery paths must wait for the full mailbox.
        let mut routed = pin!(pruned.request(Query).all().resolve().with_sender());
        assert!(poll!(routed.as_mut()).is_pending());
        let mut group = pin!(
            pruned
                .request_to(responders_addr, Query)
                .all()
                .resolve()
                .with_sender()
        );
        assert!(poll!(group.as_mut()).is_pending());
        let mut direct = pin!(pruned.request_to(full_addr, Query).resolve().with_sender());
        assert!(poll!(direct.as_mut()).is_pending());
        ctx.respond(resume, ());

        let (mut routed_responses, mut group_responses, (response, sender)) =
            futures::join!(routed, group, direct);
        assert_eq!(response.unwrap(), ActorKind::Full);
        assert_eq!(sender, full_addr);

        for responses in [&mut routed_responses, &mut group_responses] {
            responses.sort_by_key(|(response, _)| response.as_ref().ok().copied());
            assert!(
                matches!(
                    responses.as_slice(),
                    [
                        (Err(RequestError::Failed), closed),
                        (Ok(ActorKind::Ok), ok),
                        (Ok(ActorKind::Full), full),
                    ] if *closed == closed_addr && *ok == ok_addr && *full == full_addr
                ),
                "{responses:?}"
            );
        }

        // Any succeeds despite the closed mailbox being visited first.
        let (response, sender) = ctx.request(Query).resolve().with_sender().await;
        match response.unwrap() {
            ActorKind::Ok => assert_eq!(sender, ok_addr),
            ActorKind::Full => assert_eq!(sender, full_addr),
            ActorKind::ClosedMailbox => unreachable!(),
        }

        // The queued request fails on termination with the actor's address.
        ctx.respond(terminate, ());
        let (response, sender) = queued.await;
        assert!(matches!(response, Err(RequestError::Failed)));
        assert_eq!(sender, closed_addr);

        ctx.respond(done, ());
    }

    common::setup_logger();
    let topology = Topology::empty();
    let configurers = topology.local("system.configurers").entrypoint();
    let requesters = topology.local("requesters");
    let requester_addr = requesters.addr();
    let responders = topology.local("responders");
    let responders_addr = responders.addr();
    requesters.route_all_to(&responders);
    responders.route_all_to(&requesters);

    configurers.mount(elfo_configurer::fixture(&topology, AnyConfig::default()));
    requesters.mount(ActorGroup::new().exec(move |ctx| requester(ctx, responders_addr)));
    responders.mount(
        ActorGroup::new()
            .router(MapRouter::new(|envelope| {
                msg!(match envelope {
                    Spawn(key) => Outcome::Unicast(*key),
                    Ping => Outcome::Unicast(ActorKind::ClosedMailbox),
                    Query => Outcome::Multicast(vec![
                        ActorKind::ClosedMailbox,
                        ActorKind::Ok,
                        ActorKind::Full,
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
