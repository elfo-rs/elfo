use std::time::{Duration, Instant};

use anyhow::ensure;
use criterion::{criterion_group, criterion_main, Criterion, Throughput};
use tokio::runtime::Runtime;

use elfo::{
    config::AnyConfig,
    messages::UpdateConfig,
    prelude::*,
    routers::{MapRouter, Outcome},
    topology::{GetAddrs, Topology},
};

#[message]
struct Command {
    value: u32,
}

#[message]
struct Event {
    value: u32,
}

#[message(ret = Response)]
struct Request {
    value: u32,
}

#[message]
struct Response {
    value: u32,
}

#[message(ret = Duration)]
struct Summarize;

fn make_producers(count: u32, iter_count: u32) -> Schema {
    ActorGroup::new()
        .router(MapRouter::new(move |envelope| {
            msg!(match envelope {
                UpdateConfig => Outcome::Multicast((0..count).collect()),
                Summarize => Outcome::Broadcast,
                _ => Outcome::Default,
            })
        }))
        .exec(move |mut ctx| async move {
            let start_at = Instant::now();
            let i = *ctx.key();

            for value in i..(iter_count + i) {
                let res = ctx.request(Request { value }).resolve().await.unwrap();
                ensure!(res.value == value);
            }

            let spent = start_at.elapsed();

            while let Some(envelope) = ctx.recv().await {
                msg!(match envelope {
                    (Summarize, token) => ctx.respond(token, spent),
                });
            }

            Ok(())
        })
}

fn make_consumers(count: u32) -> Schema {
    ActorGroup::new()
        .router(MapRouter::new(move |envelope| {
            msg!(match envelope {
                UpdateConfig => Outcome::Multicast((0..count).collect()),
                Command { value, .. } | Request { value, .. } => Outcome::Unicast(value % count),
                _ => Outcome::Default,
            })
        }))
        .exec(move |mut ctx| async move {
            while let Some(envelope) = ctx.recv().await {
                let sender = envelope.sender();
                msg!(match envelope {
                    Command { value } => {
                        ctx.send_to(sender, Event { value }).await.unwrap();
                    }
                    (Request { value }, token) => {
                        ctx.respond(token, Response { value });
                    }
                })
            }
        })
}

async fn run(producer_count: u32, consumer_count: u32, iter_count: u32) -> Duration {
    let topology = Topology::empty();
    let producers = topology.local("producers");
    let consumers = topology.local("consumers");
    let configurers = topology.local("system.configurers").entrypoint();

    producers.route_all_to(&consumers);

    let producers_addr = producers.addrs()[0];

    producers.mount(make_producers(producer_count, iter_count));
    consumers.mount(make_consumers(consumer_count));
    configurers.mount(elfo::configurer::fixture(&topology, AnyConfig::default()));

    elfo::_priv::do_start(topology, |ctx| async move {
        ctx.request(Summarize)
            .from(producers_addr)
            .all()
            .resolve()
            .await
            .into_iter()
            .map(|spent| spent.unwrap_or_else(|_| Duration::new(0, 0))) // FIXME: should be error.
            .max()
            .unwrap()
    })
    .await
    .unwrap()
}

fn round_trip(c: &mut Criterion) {
    let mut group = c.benchmark_group("round_trip");

    let space = vec![(1, 1), (2, 2), (3, 3), (4, 4), (5, 5)];

    for (p, c) in space {
        group.throughput(Throughput::Elements(p as u64));
        group.bench_function(format!("{}/{}", p, c), |b| {
            b.iter_custom(|iter_count| {
                // let mut h = Vec::new();
                // for _ in 0..p {
                // h.push(std::thread::spawn(move || {
                // let rt = Runtime::new().unwrap();
                // rt.block_on(run(1, 1, iter_count as u32))
                //}));
                //}

                // let mut max = Duration::new(0, 0);
                // for i in h {
                // max = max.max(i.join().unwrap());
                //}
                // max

                let rt = Runtime::new().unwrap();
                rt.block_on(run(p, c, iter_count as u32))
            })
        });
    }
    group.finish();
}

criterion_group!(benches, round_trip);
criterion_main!(benches);
