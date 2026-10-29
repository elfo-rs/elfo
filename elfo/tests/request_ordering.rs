#![allow(missing_docs)]
#![cfg(feature = "test-util")]

use std::{future::Future, task::Poll, time::Duration};

use itertools::Itertools;
#[cfg(all(feature = "network", feature = "turmoil07"))]
use toml::toml;

use elfo::{
    _priv::do_start,
    Topology,
    config::AnyConfig,
    errors::RequestError,
    messages::Ping,
    prelude::*,
    routers::{MapRouter, Outcome},
    test::extract_request,
};

mod common;

#[test]
fn all_collects_responses() {
    async fn requester(ctx: Context) {
        let requests = ctx.pruned();
        let responses = [
            Response::Ok(42),
            Response::Ignore,
            Response::Err,
            Response::Ok(43),
        ];
        for order in (0..responses.len()).permutations(responses.len()) {
            let scenario = format!("responses: {responses:?}, order: {order:?}");
            tracing::debug!(%scenario, "starting response order");
            prepare_responses(&ctx, &responses).await;
            let mut query =
                std::pin::pin!(requests.request(Query(responses.len())).all().resolve());
            assert!(futures::poll!(&mut query).is_pending(), "{scenario}");

            for (step, &key) in order.iter().enumerate() {
                ctx.request(MakeResponse(key))
                    .resolve()
                    .await
                    .expect(&scenario);
                // The request must remain pending until all actors respond.
                if step < order.len() - 1 {
                    assert!(
                        futures::poll!(&mut query).is_pending(),
                        "{scenario}, step: {step}"
                    );
                }
            }

            let results = query.await;
            tracing::debug!(%scenario, ?results, "query completed");
            assert_eq!(results.len(), order.len(), "{scenario}");
            for (response, key) in results.iter().zip(order) {
                assert_response(response, responses[key], &scenario);
            }
        }
    }

    run(requester);
}

#[test]
fn any_returns_first_success() {
    async fn requester(ctx: Context) {
        let requests = ctx.pruned();
        let responses = [
            Response::Ok(42),
            Response::Ignore,
            Response::Err,
            Response::Ok(43),
        ];
        for order in (0..responses.len()).permutations(responses.len()) {
            let scenario = format!("responses: {responses:?}, order: {order:?}");
            tracing::debug!(%scenario, "starting response order");
            prepare_responses(&ctx, &responses).await;
            let mut query = std::pin::pin!(requests.request(Query(responses.len())).resolve());
            assert!(futures::poll!(&mut query).is_pending(), "{scenario}");
            let first_ok = order
                .iter()
                .position(|&key| matches!(responses[key], Response::Ok(_)))
                .unwrap();

            for (step, &key) in order.iter().enumerate() {
                ctx.request(MakeResponse(key))
                    .resolve()
                    .await
                    .expect(&scenario);
                if step < first_ok {
                    assert!(
                        futures::poll!(&mut query).is_pending(),
                        "{scenario}, step: {step}"
                    );
                } else if step == first_ok {
                    let Poll::Ready(result) = futures::poll!(&mut query) else {
                        panic!("request is still pending after the first success; {scenario}");
                    };
                    tracing::debug!(%scenario, ?result, "query completed");
                    assert_response(&result, responses[key], &scenario);
                }
            }
        }
    }

    run(requester);
}

#[test]
fn any_waits_for_all_errors() {
    async fn requester(ctx: Context) {
        let requests = ctx.pruned();
        for (responses, expected) in [
            ([Response::Ignore, Response::Ignore], Response::Ignore),
            ([Response::Err, Response::Err], Response::Err),
            ([Response::Ignore, Response::Err], Response::Ignore),
        ] {
            for order in (0..responses.len()).permutations(responses.len()) {
                let scenario = format!("responses: {responses:?}, order: {order:?}");
                tracing::debug!(%scenario, "starting response order");
                prepare_responses(&ctx, &responses).await;
                let mut query = std::pin::pin!(requests.request(Query(responses.len())).resolve());
                assert!(futures::poll!(&mut query).is_pending(), "{scenario}");

                for (step, &key) in order.iter().enumerate() {
                    ctx.request(MakeResponse(key))
                        .resolve()
                        .await
                        .expect(&scenario);
                    if step < order.len() - 1 {
                        assert!(
                            futures::poll!(&mut query).is_pending(),
                            "{scenario}, step: {step}"
                        );
                    }
                }
                let result = query.await;
                tracing::debug!(%scenario, ?result, "query completed");
                assert_response(&result, expected, &scenario);
            }
        }
    }

    run(requester);
}

#[message(part)]
#[derive(Copy)]
enum Response {
    Ok(u64),
    Ignore,
    Err,
}

#[message(ret = ())]
struct RunTest;
#[message(ret = u64)]
struct Query(usize);
#[message(ret = ())]
struct MakeResponse(usize);
#[message(ret = ())]
struct PrepareResponse(usize, Response);

async fn prepare_responses(ctx: &Context, responses: &[Response]) {
    for (key, &response) in responses.iter().enumerate() {
        ctx.request(PrepareResponse(key, response))
            .resolve()
            .await
            .unwrap();
    }
}

async fn responder(mut ctx: Context<(), usize>) {
    while let Some(prepare) = ctx.recv().await {
        let (PrepareResponse(_, response), command) = extract_request(prepare);
        ctx.respond(command, ());

        let request = ctx.recv().await.unwrap();
        assert!(request.is::<Query>());

        let (MakeResponse(_), command) = extract_request(ctx.recv().await.unwrap());
        match response {
            Response::Ok(value) => {
                let (Query(_), token) = extract_request(request);
                ctx.respond(token, value);
            }
            Response::Ignore => {
                let (Query(_), token) = extract_request(request);
                drop(token);
            }
            // Drop the envelope without extracting the token to produce `Failed`.
            Response::Err => drop(request),
        }
        // The acknowledgment follows the query response.
        ctx.respond(command, ());
    }
}

#[track_caller]
fn assert_response(response: &Result<u64, RequestError>, expected: Response, scenario: &str) {
    match expected {
        Response::Ok(expected) => assert!(
            matches!(response, Ok(value) if *value == expected),
            "{scenario}, expected: {expected:?}, response: {response:?}"
        ),
        Response::Ignore => assert!(
            matches!(response, Err(RequestError::Ignored)),
            "{scenario}, expected: {expected:?}, response: {response:?}"
        ),
        Response::Err => assert!(
            matches!(response, Err(RequestError::Failed)),
            "{scenario}, expected: {expected:?}, response: {response:?}"
        ),
    }
}

fn responder_blueprint() -> Blueprint {
    ActorGroup::new()
        .router(MapRouter::new(|envelope| {
            msg!(match envelope {
                Ping => Outcome::Unicast(0),
                PrepareResponse(key, _) | MakeResponse(key) => Outcome::Unicast(*key),
                Query(count) => Outcome::Multicast((0..*count).collect()),
                _ => Outcome::Default,
            })
        }))
        .exec(responder)
}

fn requester_blueprint<F>(requester: fn(Context) -> F) -> Blueprint
where
    F: Future<Output = ()> + Send + 'static,
{
    ActorGroup::new().exec(move |mut ctx| async move {
        let (RunTest, done) = extract_request(ctx.recv().await.unwrap());
        while ctx.request(Ping::default()).resolve().await.is_err() {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        let completion = ctx.pruned();
        requester(ctx).await;
        completion.respond(done, ());
    })
}

fn run<F>(requester: fn(Context) -> F)
where
    F: Future<Output = ()> + Send + 'static,
{
    common::setup_logger();
    tracing::info!(topology = "local", "starting topology");
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let topology = Topology::empty();
            let configurers = topology.local("system.configurers").entrypoint();
            let requesters = topology.local("requesters");
            let requester_addr = requesters.addr();
            let responders = topology.local("responders");
            requesters.route_all_to(&responders);
            configurers.mount(elfo::batteries::configurer::fixture(
                &topology,
                AnyConfig::default(),
            ));
            responders.mount(responder_blueprint());
            requesters.mount(requester_blueprint(requester));
            do_start(topology, false, |ctx, _| async move {
                ctx.request_to(requester_addr, RunTest)
                    .resolve()
                    .await
                    .expect("local requester failed to complete the test");
            })
            .await
            .expect("cannot start local topology");
        });

    #[cfg(all(feature = "network", feature = "turmoil07"))]
    run_remote(requester);
}

#[cfg(all(feature = "network", feature = "turmoil07"))]
fn run_remote<F>(requester: fn(Context) -> F)
where
    F: Future<Output = ()> + Send + 'static,
{
    tracing::info!(topology = "remote", "starting topology");
    let mut sim = turmoil::Builder::new().enable_tokio_io().build();

    sim.host("server", || async {
        let topology = Topology::empty();
        let configurers = topology.local("system.configurers").entrypoint();
        let network = topology.local("system.network");
        let responder_group = topology.local("responders");
        network.mount(elfo::batteries::network::new(&topology));
        configurers.mount(elfo::batteries::configurer::fixture(
            &topology,
            toml! {
                [system.network]
                listen = ["turmoil07://0.0.0.0"]
            },
        ));
        responder_group.mount(responder_blueprint());
        Ok(elfo::init::try_start(topology).await?)
    });

    sim.client("client", async move {
        let topology = Topology::empty();
        let configurers = topology.local("system.configurers").entrypoint();
        let network = topology.local("system.network");
        let requesters = topology.local("requesters");
        let requester_addr = requesters.addr();
        let responders = topology.remote("responders");
        requesters.route_to(&responders, |_, _| elfo::topology::Outcome::Broadcast);
        network.mount(elfo::batteries::network::new(&topology));
        configurers.mount(elfo::batteries::configurer::fixture(
            &topology,
            toml! {
                [system.network]
                discovery.predefined = ["turmoil07://server"]
            },
        ));
        requesters.mount(requester_blueprint(requester));
        Ok(elfo::_priv::do_start(topology, false, |ctx, _| async move {
            ctx.request_to(requester_addr, RunTest)
                .resolve()
                .await
                .expect("remote requester failed to complete the test");
        })
        .await?)
    });
    sim.run().expect("remote topology failed");
}
