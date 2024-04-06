use std::{
    env,
    str::FromStr,
    sync::{Arc, Barrier},
    thread,
    time::{Duration, Instant},
};

use criterion::{
    criterion_group, criterion_main, measurement::WallTime, BenchmarkGroup, BenchmarkId, Criterion,
};
use metrics::{counter, gauge, histogram};
use tokio::{
    runtime::Builder,
    sync::{mpsc, oneshot},
    task,
};
use toml::toml;

use elfo_core::{
    message, msg,
    routers::{MapRouter, Outcome},
    scope, ActorGroup, Blueprint, Local, Topology,
};

// === Harness ===

#[message(ret = Duration)]
struct Bench {
    contention: u32,
    barrier: Local<Arc<Barrier>>,
    iter_count: u32,
    testee: Local<Arc<dyn Fn(f64) + Send + Sync>>,
}

fn subject() -> Blueprint {
    ActorGroup::new()
        .router(MapRouter::new(move |envelope| {
            msg!(match envelope {
                msg @ Bench => Outcome::Multicast((0..msg.contention).collect()),
                _ => Outcome::Default,
            })
        }))
        .exec(move |mut ctx| async move {
            while let Some(envelope) = ctx.recv().await {
                msg!(match envelope {
                    (msg @ Bench, token) => {
                        let bench = move || {
                            msg.barrier.wait();

                            let start = Instant::now();
                            for i in 0..msg.iter_count {
                                (msg.testee)(i as f64);
                            }
                            start.elapsed()
                        };

                        let scope = scope::expose();
                        let spent = task::spawn_blocking(move || scope.sync_within(bench))
                            .await
                            .unwrap();

                        ctx.respond(token, spent);
                    }
                    _ => unreachable!(),
                });
            }
        })
}

fn setup() -> mpsc::UnboundedSender<(Bench, oneshot::Sender<Duration>)> {
    let topology = Topology::empty();
    let telemeter = elfo_telemeter::init();

    let subjects = topology.local("subjects");
    let telemeters = topology.local("system.telemeters");
    let configurers = topology.local("system.configurers").entrypoint();

    let subjects_addr = subjects.addr();

    subjects.mount(subject());
    telemeters.mount(telemeter);
    configurers.mount(elfo_configurer::fixture(
        &topology,
        toml! {
            [system.telemeters]
            sink = "Prometheus"
            listen = "127.0.0.1:9042"
        },
    ));

    let (tx, mut rx) = mpsc::unbounded_channel::<(Bench, oneshot::Sender<Duration>)>();

    thread::spawn(move || {
        let fut = elfo_core::_priv::do_start(topology, false, move |ctx, _| async move {
            while let Some((bench, otx)) = rx.recv().await {
                let spent = ctx
                    .request_to(subjects_addr, bench)
                    .all()
                    .resolve()
                    .await
                    .into_iter()
                    .map(|ts| ts.unwrap())
                    .max()
                    .unwrap();

                otx.send(spent).unwrap();
            }
        });

        let rt = Builder::new_multi_thread()
            .enable_all()
            .worker_threads(1) // `spawn_blocking` is used
            .build()
            .unwrap();

        rt.block_on(fut).unwrap();
    });

    tx
}

fn case(
    group: &mut BenchmarkGroup<'_, WallTime>,
    tx: &mpsc::UnboundedSender<(Bench, oneshot::Sender<Duration>)>,
    name: &str,
    contention: u32,
    testee: impl Fn(f64) + Send + Sync + 'static,
) {
    let testee = Arc::new(testee) as Arc<dyn Fn(f64) + Send + Sync>;
    group.bench_with_input(BenchmarkId::new(name, contention), &contention, |b, _| {
        b.iter_custom(|iter_count| {
            let bench = Bench {
                contention,
                barrier: Arc::new(Barrier::new(contention as usize)).into(),
                iter_count: iter_count as u32,
                testee: testee.clone().into(),
            };

            let (otx, orx) = oneshot::channel();
            tx.send((bench, otx)).unwrap();
            orx.blocking_recv().unwrap()
        })
    });
}

fn max_parallelism() -> u32 {
    env::var("ELFO_BENCH_MAX_PARALLELISM")
        .ok()
        .map(|s| u32::from_str(&s).expect("invalid value for ELFO_BENCH_MAX_PARALLELISM"))
        .unwrap_or_else(|| {
            usize::from(thread::available_parallelism().expect("cannot get available parallelism"))
                as u32
        })
}

// === Cases ===

// Telemeter cannot be installed in the same process several times.
// TODO: use local recorders after updating the `metrics` crate.

fn all_cases(c: &mut Criterion) {
    let tx = setup();

    let mut group = c.benchmark_group("telemetry");

    for contention in 1..=max_parallelism() {
        case(&mut group, &tx, "gauge", contention, |v| {
            gauge!("prefix_some_more_realistic_name", v)
        });
        case(&mut group, &tx, "counter", contention, |v| {
            counter!("prefix_some_more_realistic_name", v as u64)
        });
        case(&mut group, &tx, "histogram", contention, |v| {
            histogram!("prefix_some_more_realistic_name", v)
        });
    }
}

criterion_group!(cases, all_cases);
criterion_main!(cases);
