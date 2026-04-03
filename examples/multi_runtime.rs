//! This example demonstrates how to use multiple tokio runtimes within
//! one elfo system to isolate different actor groups. [The Actoromicon].
//!
//! Run it as
//! ```sh
//! cargo run --bin multi_runtime --features unstable
//! ```
//!
//! [The Actoromicon]: https://actoromicon.rs/ch07-02-multiple-runtimes.html

use std::time::{Duration, Instant};

use elfo::{Topology, config::AnyConfig, prelude::*, time::Interval};
use tokio::runtime as rt;

#[message(ret = ())]
struct DoWork;

fn producer() -> Blueprint {
    #[message]
    struct Tick;

    ActorGroup::new().exec(|mut ctx| async move {
        ctx.attach(Interval::new(Tick))
            .start(Duration::from_millis(100));

        while let Some(envelope) = ctx.recv().await {
            msg!(match envelope {
                Tick => {
                    tracing::info!(message = "requesting some work", thread = get_thread_name());
                    ctx.request(DoWork).resolve().await.unwrap();
                    tracing::info!(message = "got results", thread = get_thread_name());
                }
            });
        }
    })
}

fn worker() -> Blueprint {
    ActorGroup::new().exec(|mut ctx| async move {
        while let Some(envelope) = ctx.recv().await {
            msg!(match envelope {
                (DoWork, token) => {
                    let start = Instant::now();

                    while start.elapsed() < Duration::from_millis(50) {
                        std::hint::spin_loop();
                    }

                    tracing::info!(
                        message = "completed some work",
                        elapsed = ?start.elapsed(),
                        thread = get_thread_name(),
                    );

                    ctx.respond(token, ());
                }
            });
        }
    })
}

fn get_thread_name() -> String {
    std::thread::current().name().unwrap_or("").into()
}

fn topology() -> (Topology, Vec<rt::Runtime>) {
    let producers_rt = start_runtime("producers", 1);
    let workers_rt = start_runtime("workers", 3);

    let topology = Topology::empty();

    // Register dedicated runtimes with filters.
    // Filters are checked in order, first match wins.
    // If no dedicated runtime matches, the actor is spawned on the default runtime.
    topology.add_dedicated_rt(
        |meta| meta.group == "producers",
        producers_rt.handle().clone(),
    );
    topology.add_dedicated_rt(|meta| meta.group == "workers", workers_rt.handle().clone());

    let logger = elfo::batteries::logger::init();

    let producers = topology.local("producers");
    let workers = topology.local("workers");
    let loggers = topology.local("system.loggers");
    let configurers = topology.local("system.configurers").entrypoint();

    producers.route_all_to(&workers);

    producers.mount(producer());
    workers.mount(worker());
    loggers.mount(logger);
    configurers.mount(elfo::batteries::configurer::fixture(
        &topology,
        AnyConfig::default(),
    ));

    (topology, vec![producers_rt, workers_rt])
}

fn start_runtime(name: &str, workers: usize) -> rt::Runtime {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let name = name.to_string();
    let worker_idx = AtomicUsize::new(0);

    rt::Builder::new_multi_thread()
        .worker_threads(workers)
        .enable_all()
        .thread_name_fn(move || {
            let idx = worker_idx.fetch_add(1, Ordering::SeqCst);
            format!("{name}#{idx}")
        })
        .on_thread_start(|| {
            // A good place to configure thread affinity and set scheduler
            // policy, e.g. `SCHED_FIFO`.
            //
            // Count threads in order to distinguish between workers and
            // blocking ones: first `workers` threads are workers, the rest are
            // blocking threads.
        })
        .build()
        .unwrap()
}

fn main() {
    let (topology, runtimes) = topology();

    start_runtime("default", 1).block_on(elfo::init::start(topology));

    for rt in runtimes {
        rt.shutdown_background();
    }
}
