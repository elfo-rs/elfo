use std::{
    hint::black_box,
    time::{Duration, Instant},
};

use criterion::{BenchmarkId, Criterion, criterion_group};

use elfo::{Addr, Local, config::AnyConfig, prelude::*, topology::Topology};

#[path = "common.rs"]
mod common;

// === Messages ===

#[message]
struct Sample {
    value: u32,
}

#[message(ret = Local<Addr>)]
struct ResolveAddr;

#[message(ret = Local<Instant>)]
struct Summarize;

// === Send methods ===

#[derive(Clone, Copy)]
enum SendMethod {
    SendRouted,
    TrySendRouted,
    SendDirect,
    TrySendDirect,
}

impl SendMethod {
    fn name(self) -> &'static str {
        match self {
            SendMethod::SendRouted => "send_routed",
            SendMethod::TrySendRouted => "try_send_routed",
            SendMethod::SendDirect => "send_direct",
            SendMethod::TrySendDirect => "try_send_direct",
        }
    }
}

// === Actors ===

fn make_producer(method: SendMethod, iter_count: u32, group_count: usize) -> Blueprint {
    ActorGroup::new().exec(move |mut ctx| async move {
        // Resolve addresses of all consumers.
        // This also ensures all consumers are spawned before we start sending.
        let consumer_addrs = ctx
            .request(ResolveAddr)
            .all()
            .resolve()
            .await
            .into_iter()
            .map(|res| res.unwrap().into_inner())
            .collect::<Vec<_>>();
        assert_eq!(consumer_addrs.len(), group_count);

        let start_at = Instant::now();

        for i in 0..iter_count {
            let sample = Sample { value: i };
            match method {
                SendMethod::SendRouted => {
                    ctx.send(sample).await.unwrap();
                }
                SendMethod::TrySendRouted => {
                    ctx.try_send(sample).unwrap();
                }
                SendMethod::SendDirect => {
                    let addr = consumer_addrs[i as usize % group_count];
                    ctx.send_to(addr, sample).await.unwrap();
                }
                SendMethod::TrySendDirect => {
                    let addr = consumer_addrs[i as usize % group_count];
                    ctx.try_send_to(addr, sample).unwrap();
                }
            }
        }

        msg!(match ctx.recv().await.unwrap() {
            (Summarize, token) => ctx.respond(token, start_at.into()),
            _ => unreachable!(),
        })
    })
}

fn make_consumer() -> Blueprint {
    ActorGroup::new().exec(move |mut ctx| async move {
        ctx.set_mailbox_capacity(100_000_000);

        while let Some(envelope) = ctx.recv().await {
            msg!(match envelope {
                msg @ Sample => {
                    black_box(msg);
                }
                (ResolveAddr, token) => {
                    ctx.respond(token, ctx.addr().into());
                }
                (Summarize, token) => {
                    ctx.respond(token, Instant::now().into());
                    return;
                }
            });
        }
    })
}

// === Harness ===

async fn run(method: SendMethod, group_count: usize, iter_count: u32) -> Duration {
    let topology = Topology::empty();
    let producer = topology.local("producer");
    let configurers = topology.local("system.configurers").entrypoint();

    let mut consumer_addrs = Vec::with_capacity(group_count);

    for i in 0..group_count {
        let name = format!("consumer_{i}");
        let consumer = topology.local(&name);
        let gc = group_count;
        producer.route_to(&consumer, move |envelope| {
            msg!(match envelope {
                Sample { value, .. } => *value as usize % gc == i,
                _ => true,
            })
        });
        consumer_addrs.push(consumer.addr());
        consumer.mount(make_consumer());
    }

    let producer_addr = producer.addr();
    producer.mount(make_producer(method, iter_count, group_count));

    configurers.mount(elfo::batteries::configurer::fixture(
        &topology,
        AnyConfig::default(),
    ));

    elfo::_priv::do_start(topology, false, |ctx, _| async move {
        // Wait for producer to finish sending.
        let start = ctx
            .request_to(producer_addr, Summarize)
            .all()
            .resolve()
            .await
            .into_iter()
            .map(|ts| ts.unwrap().into_inner())
            .min()
            .unwrap();

        // Wait for all consumers to drain.
        let latest = consumer_addrs.iter().map(|&addr| {
            let ctx = ctx.pruned();
            async move {
                ctx.request_to(addr, Summarize)
                    .all()
                    .resolve()
                    .await
                    .into_iter()
                    .map(|ts| ts.unwrap().into_inner())
                    .max()
                    .unwrap()
            }
        });

        let end = futures::future::join_all(latest)
            .await
            .into_iter()
            .max()
            .unwrap();

        end - start
    })
    .await
    .unwrap()
}

// === Benchmark functions ===

fn bench_method(c: &mut Criterion, method: SendMethod) {
    let workers = common::tokio_worker_threads();
    let mut group = c.benchmark_group(method.name());

    for group_count in group_counts() {
        group.bench_with_input(
            BenchmarkId::from_parameter(format!("{group_count}g{workers}w")),
            &group_count,
            |b, &gc| {
                b.iter_custom(|iter_count| {
                    let rt = common::make_mt_runtime(workers);
                    let elapsed = rt.block_on(run(method, gc, iter_count as u32));
                    rt.shutdown_timeout(Duration::from_secs(10));
                    elapsed
                })
            },
        );
    }
    group.finish();
}

fn send_routed(c: &mut Criterion) {
    bench_method(c, SendMethod::SendRouted);
}

fn try_send_routed(c: &mut Criterion) {
    bench_method(c, SendMethod::TrySendRouted);
}

fn send_direct(c: &mut Criterion) {
    bench_method(c, SendMethod::SendDirect);
}

fn try_send_direct(c: &mut Criterion) {
    bench_method(c, SendMethod::TrySendDirect);
}

fn group_counts() -> Vec<usize> {
    vec![1, 2, 5, 10, 20, 30, 50, 75, 100]
}

criterion_group!(
    cases,
    send_routed,
    try_send_routed,
    send_direct,
    try_send_direct
);
