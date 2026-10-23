#![allow(missing_docs)]
#![cfg(all(feature = "network", feature = "turmoil07"))]

use std::time::Duration;

use futures::{future::join_all, poll};
use toml::toml;

use elfo::{
    Topology,
    errors::RequestError,
    messages::Ping,
    prelude::*,
    routers::{MapRouter, Outcome},
    topology,
};
use elfo_test::extract_request;

mod common;

#[test]
fn with_sender() {
    elfo::set_protocol!("remote_requests::with_sender");

    #[message(part)]
    #[derive(Copy, PartialEq, Eq, Hash, derive_more::Display)]
    enum ActorKind {
        A,
        B,
    }

    #[message(ret = ActorKind)]
    struct Query;

    #[message(ret = ActorKind)]
    struct WhoAmI;

    #[message(ret = ())]
    struct RunTest;

    async fn responder(mut ctx: Context<(), ActorKind>) {
        while let Some(envelope) = ctx.recv().await {
            msg!(match envelope {
                (Query, token) => match ctx.key() {
                    ActorKind::A => ctx.respond(token, ActorKind::A),
                    ActorKind::B => drop(token),
                },
                (WhoAmI, token) => ctx.respond(token, *ctx.key()),
                _ => unreachable!(),
            });
        }
    }

    async fn requester(mut ctx: Context) {
        let (RunTest, done) = extract_request(ctx.recv().await.unwrap());

        while ctx.request(Ping::default()).resolve().await.is_err() {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        let responses = ctx.request(Query).all().resolve().with_sender().await;
        let mut results = Vec::new();
        for (response, sender) in responses {
            assert!(sender.is_remote());
            let key = ctx.request_to(sender, WhoAmI).resolve().await.unwrap();
            results.push((key, response));
        }
        results.sort_by_key(|(_, response)| response.is_err());
        assert!(matches!(
            results.as_slice(),
            [
                (ActorKind::A, Ok(ActorKind::A)),
                (ActorKind::B, Err(RequestError::Ignored)),
            ]
        ));

        turmoil::partition("client", "server");
        let (response, sender) = ctx.request(Query).resolve().with_sender().await;
        assert!(matches!(response, Err(RequestError::Failed)));
        assert!(sender.is_null());

        ctx.respond(done, ());
    }

    common::setup_logger();
    let mut sim = turmoil::Builder::new()
        .enable_tokio_io()
        .tick_duration(Duration::from_millis(100))
        .build();

    sim.host("server", || async {
        let topology = Topology::empty();
        let configurers = topology.local("system.configurers").entrypoint();
        let network = topology.local("system.network");
        let responders = topology.local("responders");
        network.mount(elfo::batteries::network::new(&topology));
        configurers.mount(elfo::batteries::configurer::fixture(
            &topology,
            toml! {
                [system.network]
                listen = ["turmoil07://0.0.0.0"]
                ping_interval = "100ms"
                idle_timeout = "500ms"
            },
        ));
        responders.mount(
            ActorGroup::new()
                .router(MapRouter::new(|envelope| {
                    msg!(match envelope {
                        Ping => Outcome::Unicast(ActorKind::A),
                        Query => Outcome::Multicast(vec![ActorKind::A, ActorKind::B]),
                        _ => Outcome::Default,
                    })
                }))
                .exec(responder),
        );
        Ok(elfo::init::try_start(topology).await?)
    });

    sim.client("client", async {
        let topology = Topology::empty();
        let configurers = topology.local("system.configurers").entrypoint();
        let network = topology.local("system.network");
        let requesters = topology.local("requesters");
        let requester_addr = requesters.addr();
        let responders = topology.remote("responders");
        requesters.route_to(&responders, |_, _| topology::Outcome::Broadcast);
        network.mount(elfo::batteries::network::new(&topology));
        configurers.mount(elfo::batteries::configurer::fixture(
            &topology,
            toml! {
                [system.network]
                discovery.predefined = ["turmoil07://server"]
                discovery.attempt_interval = "0ms"
                ping_interval = "100ms"
                idle_timeout = "500ms"
            },
        ));
        requesters.mount(ActorGroup::new().exec(requester));
        Ok(elfo::_priv::do_start(topology, false, |ctx, _| async move {
            ctx.request_to(requester_addr, RunTest)
                .resolve()
                .await
                .expect("requester failed to complete the test");
        })
        .await?)
    });
    sim.run().unwrap();
}

#[test]
fn full_mailbox_preserves_order() {
    use mailbox::{Command, ConfirmCommand, Query};

    async fn requester(mut ctx: Context) {
        ctx.send(Command::Resume).await.unwrap();
        let confirmation = ctx.recv().await.unwrap();
        let full_addr = confirmation.sender();
        assert!(full_addr.is_remote());
        let (ConfirmCommand, token) = extract_request(confirmation);

        let mut requests = Vec::new();
        for value in 0..4 {
            let request = if value % 2 == 0 {
                ctx.request_to(full_addr, Query(value))
            } else {
                ctx.request(Query(value))
            };
            let mut request = Box::pin(request.resolve().with_sender());
            assert!(poll!(request.as_mut()).is_pending());
            requests.push(request);
        }

        // Ping reaches Ok on the same connection while Full cannot receive.
        ctx.request(Ping::default()).resolve().await.unwrap();
        ctx.respond(token, ());

        for (expected, (response, sender)) in (0..4).zip(join_all(requests).await) {
            assert_eq!(response.unwrap(), expected);
            assert_eq!(sender, full_addr);
        }
    }

    mailbox::run(requester);
}

#[test]
fn full_mailbox_on_termination() {
    use mailbox::{Command, ConfirmCommand, Query};

    async fn requester(mut ctx: Context) {
        ctx.send(Command::Terminate).await.unwrap();
        let confirmation = ctx.recv().await.unwrap();
        let full_addr = confirmation.sender();
        assert!(full_addr.is_remote());
        let (ConfirmCommand, token) = extract_request(confirmation);

        let mut requests = Vec::new();
        for value in 0..4 {
            let request = if value % 2 == 0 {
                ctx.request_to(full_addr, Query(value))
            } else {
                ctx.request(Query(value))
            };
            let mut request = Box::pin(request.resolve().with_sender());
            assert!(poll!(request.as_mut()).is_pending());
            requests.push(request);
        }

        ctx.request(Ping::default()).resolve().await.unwrap();
        ctx.respond(token, ());

        let responses = join_all(requests).await;
        assert!(
            matches!(
                responses.as_slice(),
                [
                    (Err(RequestError::Failed), queued_in_mailbox),
                    (Err(RequestError::Failed), pushing),
                    (Err(RequestError::Failed), queued_in_flow_a),
                    (Err(RequestError::Failed), queued_in_flow_b),
                ] if *queued_in_mailbox == full_addr
                    && *pushing == full_addr
                    && queued_in_flow_a.is_null()
                    && queued_in_flow_b.is_null()
            ),
            "{responses:?}"
        );
    }

    mailbox::run(requester);
}

#[test]
fn full_mailbox_closes_during_push() {
    use mailbox::{Command, ConfirmCommand, Query};

    async fn requester(mut ctx: Context) {
        ctx.send(Command::Resume).await.unwrap();
        let confirmation = ctx.recv().await.unwrap();
        let full_addr = confirmation.sender();
        assert!(full_addr.is_remote());
        let (ConfirmCommand, resume) = extract_request(confirmation);

        // Fill the mailbox so Close becomes the pusher's first pending delivery.
        let request_ctx = ctx.pruned();
        let mut first = Box::pin(
            request_ctx
                .request_to(full_addr, Query(0))
                .resolve()
                .with_sender(),
        );
        assert!(poll!(first.as_mut()).is_pending());
        ctx.send_to(full_addr, Command::Close).await.unwrap();
        ctx.send_to(full_addr, Command::Terminate).await.unwrap();

        let mut requests = vec![first];
        for value in 1..3 {
            let request = if value == 1 {
                request_ctx.request_to(full_addr, Query(value))
            } else {
                request_ctx.request(Query(value))
            };
            let mut request = Box::pin(request.resolve().with_sender());
            assert!(poll!(request.as_mut()).is_pending());
            requests.push(request);
        }
        ctx.request(Ping::default()).resolve().await.unwrap();
        ctx.respond(resume, ());

        // Close has arrived: the first push succeeded. Terminate now fills the mailbox.
        let confirmation = ctx.recv().await.unwrap();
        assert_eq!(confirmation.sender(), full_addr);
        let (ConfirmCommand, close) = extract_request(confirmation);
        ctx.respond(close, ());

        // Keep the actor alive while the pusher fails the next delivery.
        let confirmation = ctx.recv().await.unwrap();
        assert_eq!(confirmation.sender(), full_addr);
        let (ConfirmCommand, terminate) = extract_request(confirmation);

        let responses = join_all(requests).await;
        assert!(
            matches!(
                responses.as_slice(),
                [
                    (Ok(0), responder),
                    (Err(RequestError::Failed), pushing),
                    (Err(RequestError::Failed), queued_in_flow),
                ] if *responder == full_addr
                    && *pushing == full_addr
                    && queued_in_flow.is_null()
            ),
            "{responses:?}"
        );
        ctx.respond(terminate, ());
    }

    mailbox::run(requester);
}

#[test]
fn full_mailbox_disconnect() {
    use mailbox::{Command, ConfirmCommand, Query};

    async fn requester(mut ctx: Context) {
        ctx.send(Command::Terminate).await.unwrap();
        let confirmation = ctx.recv().await.unwrap();
        let full_addr = confirmation.sender();
        assert!(full_addr.is_remote());
        let (ConfirmCommand, token) = extract_request(confirmation);

        let mut requests = Vec::new();
        for value in 0..4 {
            let request = if value % 2 == 0 {
                ctx.request_to(full_addr, Query(value))
            } else {
                ctx.request(Query(value))
            };
            let mut request = Box::pin(request.resolve().with_sender());
            assert!(poll!(request.as_mut()).is_pending());
            requests.push(request);
        }

        ctx.request(Ping::default()).resolve().await.unwrap();
        turmoil::partition("client", "server");
        ctx.respond(token, ());

        let responses = join_all(requests).await;
        assert!(
            matches!(
                responses.as_slice(),
                [
                    (Err(RequestError::Failed), queued_in_mailbox),
                    (Err(RequestError::Failed), pushing),
                    (Err(RequestError::Failed), queued_in_flow_a),
                    (Err(RequestError::Failed), queued_in_flow_b),
                ] if queued_in_mailbox.is_null()
                    && pushing.is_null()
                    && queued_in_flow_a.is_null()
                    && queued_in_flow_b.is_null()
            ),
            "{responses:?}"
        );
    }

    mailbox::run(requester);
}

#[test]
fn closed_mailbox_rejects_requests() {
    use mailbox::{Command, ConfirmCommand, Query};

    async fn requester(mut ctx: Context) {
        ctx.send(Command::Close).await.unwrap();
        let confirmation = ctx.recv().await.unwrap();
        let full_addr = confirmation.sender();
        assert!(full_addr.is_remote());
        let (ConfirmCommand, close) = extract_request(confirmation);

        // Queue Terminate before closing, then hold the actor alive.
        ctx.send_to(full_addr, Command::Terminate).await.unwrap();
        ctx.request(Ping::default()).resolve().await.unwrap();
        ctx.respond(close, ());

        let confirmation = ctx.recv().await.unwrap();
        assert_eq!(confirmation.sender(), full_addr);
        let (ConfirmCommand, terminate) = extract_request(confirmation);

        let (response, sender) = ctx
            .request_to(full_addr, Query(0))
            .resolve()
            .with_sender()
            .await;
        assert!(
            matches!(response, Err(RequestError::Failed)),
            "{response:?}"
        );
        assert_eq!(sender, full_addr);

        let (response, sender) = ctx.request(Query(1)).resolve().with_sender().await;
        assert!(
            matches!(response, Err(RequestError::Failed)),
            "{response:?}"
        );
        assert_eq!(sender, full_addr);
        ctx.respond(terminate, ());
    }

    mailbox::run(requester);
}

mod mailbox {
    use std::future::Future;

    use super::*;

    elfo::set_protocol!("remote_requests::mailbox");

    #[message(part)]
    #[derive(Copy, PartialEq, Eq, Hash, derive_more::Display)]
    pub(super) enum ActorKind {
        Full,
        Ok,
    }

    #[message]
    #[derive(Copy)]
    pub(super) enum Command {
        Resume,
        Terminate,
        Close,
    }

    #[message(ret = u32)]
    pub(super) struct Query(pub(super) u32);

    #[message(ret = ())]
    pub(super) struct ConfirmCommand;

    #[message(ret = ())]
    pub(super) struct RunTest;

    pub(super) fn run<F>(requester: fn(Context) -> F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        common::setup_logger();
        let mut sim = turmoil::Builder::new()
            .enable_tokio_io()
            .tick_duration(Duration::from_millis(100))
            .build();
        sim.host("server", || async {
            let topology = Topology::empty();
            let configurers = topology.local("system.configurers").entrypoint();
            let network = topology.local("system.network");
            let responders = topology.local("responders");
            network.mount(elfo::batteries::network::new(&topology));
            configurers.mount(elfo::batteries::configurer::fixture(
                &topology,
                toml! {
                    [system.network]
                    listen = ["turmoil07://0.0.0.0"]
                    ping_interval = "100ms"
                    idle_timeout = "500ms"
                },
            ));
            responders.mount(
                ActorGroup::new()
                    .router(MapRouter::new(|envelope| {
                        msg!(match envelope {
                            Ping => Outcome::Unicast(ActorKind::Ok),
                            Command | Query => Outcome::Unicast(ActorKind::Full),
                            _ => Outcome::Default,
                        })
                    }))
                    .exec(responder),
            );
            Ok(elfo::init::try_start(topology).await?)
        });
        sim.client("client", async move {
            let topology = Topology::empty();
            let configurers = topology.local("system.configurers").entrypoint();
            let network = topology.local("system.network");
            let requesters = topology.local("requesters");
            let requester_addr = requesters.addr();
            let responders = topology.remote("responders");
            requesters.route_to(&responders, |_, _| topology::Outcome::Broadcast);
            network.mount(elfo::batteries::network::new(&topology));
            configurers.mount(elfo::batteries::configurer::fixture(
                &topology,
                toml! {
                    [system.network]
                    discovery.predefined = ["turmoil07://server"]
                    discovery.attempt_interval = "100ms"
                    ping_interval = "100ms"
                    idle_timeout = "500ms"
                },
            ));
            requesters.mount(ActorGroup::new().exec(move |mut ctx: Context| async move {
                let (RunTest, done) = extract_request(ctx.recv().await.unwrap());
                while ctx.request(Ping::default()).resolve().await.is_err() {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }

                let reply_ctx = ctx.pruned();
                requester(ctx).await;
                reply_ctx.respond(done, ());
            }));
            Ok(elfo::_priv::do_start(topology, false, |ctx, _| async move {
                ctx.request_to(requester_addr, RunTest)
                    .resolve()
                    .await
                    .unwrap();
            })
            .await?)
        });
        sim.run().unwrap();
    }

    async fn responder(mut ctx: Context<(), ActorKind>) {
        ctx.set_mailbox_capacity(Some(1));
        let mut expected = 0;
        while let Some(envelope) = ctx.recv().await {
            let sender = envelope.sender();
            msg!(match envelope {
                command @ Command => {
                    let confirmed = ctx.request_to(sender, ConfirmCommand).resolve().await;
                    match command {
                        Command::Close => {
                            ctx.close();
                        }
                        Command::Resume => confirmed.unwrap(),
                        Command::Terminate => return,
                    }
                }
                (Query(value), token) => {
                    assert_eq!(value, expected);
                    expected += 1;
                    ctx.respond(token, value);
                }
                _ => unreachable!(),
            });
        }
    }
}

#[test]
fn failed_response_decoding_completes_request() {
    common::setup_logger();

    #[message(ret = ())]
    struct RunTest;

    #[message(ret = InvalidResponse)]
    struct Query;

    // Serialization succeeds, but the requester cannot decode the response.
    #[message]
    struct InvalidResponse(#[serde(deserialize_with = "reject_response")] ());

    fn reject_response<'de, D: serde::Deserializer<'de>>(_: D) -> Result<(), D::Error> {
        Err(serde::de::Error::custom(
            "response rejected by test decoder",
        ))
    }

    async fn responder(mut ctx: Context) {
        let (Query, token) = extract_request(ctx.recv().await.unwrap());
        ctx.respond(token, InvalidResponse(()));

        while ctx.recv().await.is_some() {}
    }

    async fn requester(mut ctx: Context) {
        let (RunTest, done) = extract_request(ctx.recv().await.unwrap());

        // Wait for discovery to register the remote route.
        while ctx.request(Ping::default()).resolve().await.is_err() {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        // A decoding error must complete the request instead of leaving it pending.
        let response = ctx.request(Query).resolve().await;
        assert!(response.unwrap_err().is_failed());
        // The connection survives the decoding error.
        ctx.request(Ping::default()).resolve().await.unwrap();

        ctx.respond(done, ());
    }

    let mut sim = turmoil::Builder::new()
        .enable_tokio_io()
        .tick_duration(Duration::from_millis(100))
        .build();

    sim.host("server", || async {
        let topology = Topology::empty();
        let configurers = topology.local("system.configurers").entrypoint();
        let network = topology.local("system.network");
        let responders = topology.local("responders");
        network.mount(elfo::batteries::network::new(&topology));
        configurers.mount(elfo::batteries::configurer::fixture(
            &topology,
            toml! {
                [system.network]
                listen = ["turmoil07://0.0.0.0"]
            },
        ));
        responders.mount(ActorGroup::new().exec(responder));
        Ok(elfo::init::try_start(topology).await?)
    });

    sim.client("client", async {
        let topology = Topology::empty();
        let configurers = topology.local("system.configurers").entrypoint();
        let network = topology.local("system.network");
        let requesters = topology.local("requesters");
        let requester_addr = requesters.addr();
        let responders = topology.remote("responders");
        requesters.route_to(&responders, |_, _| topology::Outcome::Broadcast);
        network.mount(elfo::batteries::network::new(&topology));
        configurers.mount(elfo::batteries::configurer::fixture(
            &topology,
            toml! {
                [system.network]
                discovery.predefined = ["turmoil07://server"]
            },
        ));
        requesters.mount(ActorGroup::new().exec(requester));
        Ok(elfo::_priv::do_start(topology, false, |ctx, _| async move {
            ctx.request_to(requester_addr, RunTest)
                .resolve()
                .await
                .expect("requester failed to complete the test");
        })
        .await?)
    });
    sim.run().unwrap();
}
