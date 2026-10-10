#![allow(missing_docs)]
#![cfg(all(feature = "network", feature = "turmoil07"))]

use std::time::Duration;

use toml::toml;

use elfo::{
    Topology,
    messages::Ping,
    prelude::*,
    routers::{MapRouter, Outcome},
    topology,
};

mod common;

#[test]
fn any_ignores_late_remote_errors() {
    use elfo_test::{extract_message, extract_request};

    common::setup_logger();

    #[message(ret = ())]
    struct RunTest;
    #[message(ret = u64)]
    struct Query;
    #[message]
    struct SuccessSent;
    #[message]
    struct IgnoreRequest;
    #[message]
    struct IgnoredSent;

    async fn responder(mut ctx: Context<(), &'static str>) {
        let request = ctx.recv().await.unwrap();
        let sender = request.sender();
        let (Query, token) = extract_request(request);
        match *ctx.key() {
            "A" => {
                ctx.respond(token, 42);
                ctx.send_to(sender, SuccessSent).await.unwrap();
            }
            _ => {
                // Only actor C receives IgnoreRequest. Actor B keeps its token,
                // so actor C's error is not the last response.
                let pending_request = (sender, token);
                IgnoreRequest = extract_message(ctx.recv().await.unwrap());
                drop(pending_request);
                ctx.send_to(sender, IgnoredSent).await.unwrap();
            }
        }
    }

    async fn requester(mut ctx: Context) {
        let (RunTest, done) = extract_request(ctx.recv().await.unwrap());

        while ctx
            .request(Ping::default())
            .all()
            .resolve()
            .await
            .iter()
            .all(|e| e.is_err())
        {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        // Send the request, but leave its result in the request table until both
        // markers arrive.
        let requests = ctx.pruned();
        let mut query = std::pin::pin!(requests.request(Query).resolve());
        assert!(futures::poll!(&mut query).is_pending());
        // Each marker follows its response on the same network flow.
        assert_msg!(ctx.recv().await.unwrap(), SuccessSent);

        ctx.send(IgnoreRequest).await.unwrap();
        assert_msg!(ctx.recv().await.unwrap(), IgnoredSent);

        // The request must still return the successful response after the late error.
        assert_eq!(query.await.unwrap(), 42);
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
        let responder_group = topology.local("responders");
        network.mount(elfo::batteries::network::new(&topology));
        configurers.mount(elfo::batteries::configurer::fixture(
            &topology,
            toml! {
                [system.network]
                listen = ["turmoil07://0.0.0.0"]
            },
        ));
        responder_group.mount(
            ActorGroup::new()
                .router(MapRouter::new(|envelope| {
                    msg!(match envelope {
                        Ping | Query => Outcome::Multicast(vec!["A", "B", "C"]),
                        IgnoreRequest => Outcome::Unicast("C"),
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
