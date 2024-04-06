use std::time::{Duration, Instant};

use criterion::{black_box, criterion_group, BenchmarkId, Criterion, Throughput};
use derive_more::Display;

use elfo::{
    config::AnyConfig,
    messages::UpdateConfig,
    prelude::*,
    routers::{MapRouter, Outcome},
    topology::Topology,
    Addr, Local,
};

#[path = "common.rs"]
mod common;

// === Messages ===

#[message]
struct Sample {
    value: u32,
    a: u64,
    b: f64,
    c: Option<u64>,
    d: Option<u64>,
    e: u8,
    f: f64,
    g: f64,
    h: bool,
}

impl Sample {
    fn new(value: u32) -> Self {
        Self {
            value,
            a: 42,
            b: 42.,
            c: Some(42),
            d: None,
            e: 42,
            f: 42.,
            g: 42.,
            h: true,
        }
    }
}

#[message(ret = Local<Addr>)]
struct ResolveAddrs;

#[message(ret = Local<Instant>)]
struct Summarize;

// === Flags ===

type Flags = u8;

const SEND_ROUTED: Flags = 1 << 0; // send using the routing subsystem
const SEND_DIRECT: Flags = 1 << 1; // send directly by an address

const ONE_TO_ONE: Flags = 1 << 5; // dedicated receiver for each sender
const ROUND_ROBIN: Flags = 1 << 6; // round-robin distribution
const ALL_TO_ONE: Flags = 1 << 7; // all senders to one receiver

macro_rules! flag {
    ($flag:ident) => {
        FLAGS & $flag == $flag
    };
}

macro_rules! assert_only_one_flag {
    ($($flag:ident)*) => {
        let mut count = 0;
        $(count += flag!($flag) as u8;)*
        assert_eq!(count, 1);
    };
}

// === Actors ===

fn make_producers<const FLAGS: Flags>(actor_count: u32, iter_count: u32) -> Blueprint {
    ActorGroup::new()
        .router(MapRouter::new(move |envelope| {
            msg!(match envelope {
                Summarize => Outcome::Multicast((0..actor_count).collect()),
                _ => Outcome::Default,
            })
        }))
        .exec(move |mut ctx| async move {
            let consumer_addrs = ctx
                .request(ResolveAddrs)
                .all()
                .resolve()
                .await
                .into_iter()
                .map(|res| res.unwrap().into_inner())
                .collect::<Vec<_>>();

            let key = *ctx.key();
            let start_at = Instant::now();

            for i in 0..iter_count {
                let value = if flag!(ONE_TO_ONE) {
                    key
                } else if flag!(ROUND_ROBIN) {
                    (i + key) % actor_count
                } else if flag!(ALL_TO_ONE) {
                    0
                } else {
                    unreachable!();
                };

                let sample = Sample::new(value);

                if flag!(SEND_ROUTED) {
                    ctx.send(sample).await.unwrap();
                } else if flag!(SEND_DIRECT) {
                    ctx.send_to(consumer_addrs[value as usize], sample)
                        .await
                        .unwrap();
                }

                // Yield the current task to make the benchmark more realistic.
                // Usually, actors are interrupted either by tokio's budget or by elfo's budget.
                if i % 512 == 0 {
                    tokio::task::yield_now().await;
                }
            }

            msg!(match ctx.recv().await.unwrap() {
                (Summarize, token) => ctx.respond(token, start_at.into()),
                _ => unreachable!(),
            })
        })
}

fn make_consumers<const FLAGS: Flags>(actor_count: u32) -> Blueprint {
    ActorGroup::new()
        .router(MapRouter::new(move |envelope| {
            msg!(match envelope {
                UpdateConfig | ResolveAddrs | Summarize =>
                    Outcome::Multicast((0..actor_count).collect()),
                Sample { value, .. } => {
                    assert!(*value < actor_count);
                    Outcome::Unicast(*value)
                }
                _ => Outcome::Default,
            })
        }))
        .exec(move |mut ctx| async move {
            while let Some(envelope) = ctx.recv().await {
                msg!(match envelope {
                    msg @ Sample => {
                        black_box(msg);
                    }
                    (ResolveAddrs, token) => {
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

async fn run<const FLAGS: Flags>(
    producer_count: u32,
    consumer_count: u32,
    iter_count: u32,
) -> Duration {
    let topology = Topology::empty();
    let producers = topology.local("producers");
    let consumers = topology.local("consumers");
    let configurers = topology.local("system.configurers").entrypoint();

    producers.route_all_to(&consumers);

    let producers_addr = producers.addr();
    let consumers_addr = consumers.addr();

    producers.mount(make_producers::<FLAGS>(producer_count, iter_count));
    consumers.mount(make_consumers::<FLAGS>(consumer_count));
    configurers.mount(elfo::batteries::configurer::fixture(
        &topology,
        AnyConfig::default(),
    ));

    elfo::_priv::do_start(topology, false, |ctx, _| async move {
        let earliest = ctx
            .request_to(producers_addr, Summarize)
            .all()
            .resolve()
            .await
            .into_iter()
            .map(|ts| ts.unwrap().into_inner())
            .min()
            .unwrap();

        // All producers have finished.

        let latest = ctx
            .request_to(consumers_addr, Summarize)
            .all()
            .resolve()
            .await
            .into_iter()
            .map(|ts| ts.unwrap().into_inner())
            .max()
            .unwrap();

        latest - earliest
    })
    .await
    .unwrap()
}

fn make_name<const FLAGS: Flags>() -> (&'static str, &'static str) {
    assert_only_one_flag!(SEND_ROUTED SEND_DIRECT);
    assert_only_one_flag!(ONE_TO_ONE ROUND_ROBIN ALL_TO_ONE);

    let group_id = if flag!(ONE_TO_ONE) {
        "one_to_one"
    } else if flag!(ROUND_ROBIN) {
        "round_robin"
    } else if flag!(ALL_TO_ONE) {
        "all_to_one"
    } else {
        unreachable!()
    };

    let function_id = if flag!(SEND_ROUTED) {
        "send_routed"
    } else if flag!(SEND_DIRECT) {
        "send_direct"
    } else {
        unreachable!()
    };

    (group_id, function_id)
}

fn case<const FLAGS: Flags>(c: &mut Criterion) {
    let params = CaseParams::new::<FLAGS>();

    let (group_id, function_id) = make_name::<FLAGS>();
    let mut group = c.benchmark_group(group_id);

    for p in params {
        group.throughput(Throughput::Elements(u64::from(p.producers)));

        group.bench_with_input(BenchmarkId::new(function_id, &p), &p.producers, |b, _| {
            b.iter_custom(|iter_count| {
                let rt = common::make_mt_runtime(p.workers);
                let elapsed =
                    rt.block_on(run::<FLAGS>(p.producers, p.consumers, iter_count as u32));
                rt.shutdown_timeout(Duration::from_secs(10));
                elapsed
            })
        });
    }
    group.finish();
}

#[derive(Display)]
#[display(fmt = "{}p{}c{}w", producers, consumers, workers)]
struct CaseParams {
    workers: u32,
    producers: u32,
    consumers: u32,
}

impl CaseParams {
    fn new<const FLAGS: Flags>() -> Vec<Self> {
        let max_parallelism = common::max_parallelism();
        let workers = common::tokio_worker_threads();

        (1..=10)
            .chain((12..=30).step_by(2))
            .chain((35..=60).step_by(5))
            .chain((70..).step_by(10))
            .take_while(|p| *p <= max_parallelism)
            .map(|p| {
                let producers = p;
                let consumers = if flag!(ALL_TO_ONE) { 1 } else { p };

                Self {
                    workers,
                    producers,
                    consumers,
                }
            })
            .collect()
    }
}

// === Cases ===

// Sends messages using the routing subsystem.
fn send_routed(c: &mut Criterion) {
    case::<{ SEND_ROUTED | ONE_TO_ONE }>(c);
    case::<{ SEND_ROUTED | ROUND_ROBIN }>(c);
    case::<{ SEND_ROUTED | ALL_TO_ONE }>(c);
}

// Sends messages by address of receiver.
fn send_direct(c: &mut Criterion) {
    case::<{ SEND_DIRECT | ONE_TO_ONE }>(c);
    case::<{ SEND_DIRECT | ROUND_ROBIN }>(c);
    case::<{ SEND_DIRECT | ALL_TO_ONE }>(c);
}

criterion_group!(cases, send_routed, send_direct);
