#![allow(missing_docs)]
#![cfg(all(feature = "network", feature = "turmoil07"))]

use std::time::Duration;

use toml::toml;

use elfo::{Topology, messages::Ping, prelude::*, topology};

mod common;

#[test]
fn failed_response_decoding_completes_request() {
    use elfo_test::extract_request;

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
