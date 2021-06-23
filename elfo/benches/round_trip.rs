use std::time::{Duration, Instant};

use anyhow::ensure;
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
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

#[derive(Clone, Copy)]
enum Mode {
    OnlyToOne,
    RoundRobin,
}

fn make_producers(count: u32, mode: Mode, iter_count: u32) -> Schema {
    ActorGroup::new()
        .router(MapRouter::new(move |envelope| {
            msg!(match envelope {
                Summarize => Outcome::Multicast((0..count).collect()),
                _ => Outcome::Default,
            })
        }))
        .exec(move |mut ctx| async move {
            let start_at = Instant::now();
            let key = *ctx.key();

            for i in 0..iter_count {
                let value = match mode {
                    Mode::OnlyToOne => key,
                    Mode::RoundRobin => i + key,
                };
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

async fn run(producer_count: u32, consumer_count: u32, mode: Mode, iter_count: u32) -> Duration {
    let topology = Topology::empty();
    let producers = topology.local("producers");
    let consumers = topology.local("consumers");
    let configurers = topology.local("system.configurers").entrypoint();

    producers.route_all_to(&consumers);

    let producers_addr = producers.addrs()[0];

    producers.mount(make_producers(producer_count, mode, iter_count));
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

fn one_to_one(c: &mut Criterion) {
    let mut group = c.benchmark_group("one_to_one");

    for n in 1..=12 {
        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::new("many_instances", n), &n, |b, &n| {
            b.iter_custom(|iter_count| {
                let mut h = Vec::new();
                for _ in 0..n {
                    h.push(std::thread::spawn(move || {
                        let rt = Runtime::new().unwrap();
                        rt.block_on(run(1, 1, Mode::OnlyToOne, iter_count as u32))
                    }));
                }

                let mut max = Duration::new(0, 0);
                for i in h {
                    max = max.max(i.join().unwrap());
                }
                max
            })
        });
        group.bench_with_input(BenchmarkId::new("one_instance", n), &n, |b, &n| {
            b.iter_custom(|iter_count| {
                let rt = Runtime::new().unwrap();
                rt.block_on(run(n, n, Mode::OnlyToOne, iter_count as u32))
            })
        });
    }
    group.finish();
}

fn all_to_all(c: &mut Criterion) {
    let mut group = c.benchmark_group("all_to_all");

    for n in 1..=12 {
        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::new("one_instance", n), &n, |b, &n| {
            b.iter_custom(|iter_count| {
                let rt = Runtime::new().unwrap();
                rt.block_on(run(n, n, Mode::RoundRobin, iter_count as u32))
            })
        });
    }
    group.finish();
}

criterion_group!(benches, one_to_one, all_to_all);
criterion_main!(benches);
