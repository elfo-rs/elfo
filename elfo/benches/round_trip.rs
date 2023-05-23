use std::time::{Duration, Instant};

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use tokio::runtime::Runtime;

use elfo::{
    config::AnyConfig,
    messages::{Terminate, UpdateConfig},
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

// Flags.
type Flags = u8;
const ONE_INSTANCE: Flags = 1 << 0;
const MANY_INSTANCES: Flags = 1 << 1;
const SEND_REQUEST: Flags = 1 << 2;
const SEND_COMMAND: Flags = 1 << 3;
const SEND_EVENT_BACK: Flags = 1 << 4;
const ONE_TO_ONE: Flags = 1 << 5;
const ROUND_ROBIN: Flags = 1 << 6;

macro_rules! flag {
    ($flag:ident) => {
        FLAGS & $flag == $flag
    };
}

fn make_producers<const FLAGS: Flags>(actor_count: u32, iter_count: u32) -> Blueprint {
    ActorGroup::new()
        .router(MapRouter::new(move |envelope| {
            msg!(match envelope {
                Summarize => Outcome::Multicast((0..actor_count).collect()),
                _ => Outcome::Default,
            })
        }))
        .exec(move |mut ctx| async move {
            let start_at = Instant::now();
            let key = *ctx.key();

            for i in 0..iter_count {
                let value = if flag!(ONE_TO_ONE) {
                    key
                } else if flag!(ROUND_ROBIN) {
                    i + key
                } else {
                    panic!("ONE_TO_ONE or ROUND_ROBIN must be set");
                };

                if flag!(SEND_REQUEST) {
                    let response = ctx.request(Request { value }).resolve().await.unwrap();
                    black_box(response.value);
                } else if flag!(SEND_COMMAND) {
                    ctx.send(Command { value }).await.unwrap();

                    if flag!(SEND_EVENT_BACK) {
                        // FIXME
                        if let Ok(envelope) = ctx.try_recv().await {
                            msg!(match envelope {
                                event @ Event => {
                                    black_box(event.value);
                                }
                            });
                        }
                    }
                }
            }

            let spent = start_at.elapsed();

            while let Some(envelope) = ctx.recv().await {
                msg!(match envelope {
                    (Summarize, token) => ctx.respond(token, spent),
                });
            }
        })
}

fn make_consumers<const FLAGS: Flags>(actor_count: u32) -> Blueprint {
    ActorGroup::new()
        .router(MapRouter::new(move |envelope| {
            msg!(match envelope {
                UpdateConfig => Outcome::Multicast((0..actor_count).collect()),
                Command { value, .. } | Request { value, .. } =>
                    Outcome::Unicast(value % actor_count),
                _ => Outcome::Default,
            })
        }))
        .exec(move |mut ctx| async move {
            while let Some(envelope) = ctx.recv().await {
                let sender = envelope.sender();
                msg!(match envelope {
                    Command { value } => {
                        black_box(value);
                        if flag!(SEND_EVENT_BACK) {
                            ctx.send_to(sender, Event { value }).await.unwrap();
                        }
                    }
                    (Request { value }, token) => {
                        black_box(value);
                        if flag!(SEND_REQUEST) {
                            ctx.respond(token, Response { value });
                        }
                    }
                });
            }
        })
}

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

    let producers_addr = producers.addrs()[0];
    let consumers_addr = consumers.addrs()[0];

    producers.mount(make_producers::<FLAGS>(producer_count, iter_count));
    consumers.mount(make_consumers::<FLAGS>(consumer_count));
    configurers.mount(elfo::batteries::configurer::fixture(
        &topology,
        AnyConfig::default(),
    ));

    elfo::_priv::do_start(topology, |ctx, _| async move {
        let elapsed = ctx
            .request_to(producers_addr, Summarize)
            .all()
            .resolve()
            .await
            .into_iter()
            .map(|spent| spent.unwrap_or(Duration::ZERO)) // FIXME: should be error.
            .max()
            .unwrap();

        // TODO: provide a more convenient way to stop a whole system in benches/tests.
        ctx.try_send_to(producers_addr, Terminate::closing())
            .unwrap();
        ctx.try_send_to(consumers_addr, Terminate::closing())
            .unwrap();

        tokio::time::sleep(Duration::from_millis(100)).await;

        elapsed
    })
    .await
    .unwrap()
}

fn make_name<const FLAGS: Flags>() -> String {
    let mut name = Vec::new();

    if flag!(SEND_REQUEST) {
        name.push("request_response");
    } else if flag!(SEND_COMMAND) && flag!(SEND_EVENT_BACK) {
        name.push("command_event");
    } else if flag!(SEND_COMMAND) {
        name.push("only_command");
    }

    if flag!(ONE_TO_ONE) {
        name.push("one_to_one");
    } else if flag!(ROUND_ROBIN) {
        name.push("round_robin");
    }

    name.join("/")
}

fn case<const FLAGS: Flags>(c: &mut Criterion) {
    let mut group = c.benchmark_group(make_name::<FLAGS>());

    for n in 1..=12 {
        group.throughput(Throughput::Elements(n as u64));

        if flag!(ONE_INSTANCE) {
            group.bench_with_input(BenchmarkId::new("one_instance", n), &n, |b, &n| {
                b.iter_custom(|iter_count| {
                    let rt = Runtime::new().unwrap();
                    let elapsed = rt.block_on(run::<FLAGS>(n, n, iter_count as u32));
                    rt.shutdown_timeout(Duration::from_secs(10));
                    elapsed
                })
            });
        }

        if flag!(MANY_INSTANCES) {
            if flag!(ROUND_ROBIN) {
                panic!("MANY_INSTANCES and ROUND_ROBIN are incompatible for now");
            }
            group.bench_with_input(BenchmarkId::new("many_instances", n), &n, |b, &n| {
                b.iter_custom(|iter_count| {
                    let mut h = Vec::new();
                    for _ in 0..n {
                        h.push(std::thread::spawn(move || {
                            let rt = Runtime::new().unwrap();
                            let elapsed = rt.block_on(run::<FLAGS>(1, 1, iter_count as u32));
                            rt.shutdown_timeout(Duration::from_secs(10));
                            elapsed
                        }));
                    }

                    let mut max = Duration::ZERO;
                    for i in h {
                        max = max.max(i.join().unwrap());
                    }
                    max
                })
            });
        }
    }
    group.finish();
}

// Cases.
// fn request_response(c: &mut Criterion) {
//     case::<{ ONE_INSTANCE | SEND_REQUEST | ONE_TO_ONE }>(c);
//     case::<{ ONE_INSTANCE | SEND_REQUEST | ROUND_ROBIN }>(c);
// }
fn only_command(c: &mut Criterion) {
    case::<{ ONE_INSTANCE | SEND_COMMAND | ONE_TO_ONE }>(c);
    case::<{ ONE_INSTANCE | SEND_COMMAND | ROUND_ROBIN }>(c);
}
// fn command_event(c: &mut Criterion) {
//     case::<{ ONE_INSTANCE | SEND_COMMAND | SEND_EVENT_BACK | ONE_TO_ONE }>(c);
//     case::<{ ONE_INSTANCE | SEND_COMMAND | SEND_EVENT_BACK | ROUND_ROBIN }>(c);
// }

criterion_group!(
    benches,
    // request_response,
    only_command, // command_event
);
criterion_main!(benches);
