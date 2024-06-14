#![allow(dead_code)] // typical for common test/bench modules :(

use std::{env, str::FromStr, thread::available_parallelism, time::Duration};

use tokio::runtime::{Builder, Runtime};

pub(crate) fn tokio_worker_threads() -> u32 {
    env::var("TOKIO_WORKER_THREADS")
        .ok()
        .map(|s| u32::from_str(&s).expect("invalid value for TOKIO_WORKER_THREADS"))
        .unwrap_or_else(|| {
            usize::from(available_parallelism().expect("cannot get available parallelism")) as u32
        })
}

pub(crate) fn max_parallelism() -> u32 {
    env::var("ELFO_BENCH_MAX_PARALLELISM")
        .ok()
        .map(|s| u32::from_str(&s).expect("invalid value for ELFO_BENCH_MAX_PARALLELISM"))
        .unwrap_or_else(|| {
            usize::from(available_parallelism().expect("cannot get available parallelism")) as u32
        })
}

pub(crate) fn make_mt_runtime(worker_threads: u32) -> Runtime {
    use std::sync::atomic::{AtomicU32, Ordering};

    // To make it easier to check in the profiler.
    let test_no = {
        static BENCH_NO: AtomicU32 = AtomicU32::new(0);
        BENCH_NO.fetch_add(1, Ordering::Relaxed)
    };

    let next_worker_no = AtomicU32::new(0);

    Builder::new_multi_thread()
        .enable_all()
        .worker_threads(worker_threads as usize)
        .thread_name_fn(move || {
            let worker_no = next_worker_no.fetch_add(1, Ordering::Relaxed);
            format!("b{test_no}-w{worker_no}")
        })
        .build()
        .unwrap()
}

pub(crate) fn make_st_runtime() -> Runtime {
    Builder::new_current_thread().enable_all().build().unwrap()
}

pub(crate) fn bench_singleton<R>(
    iter_count: u64,
    f: impl FnOnce(elfo::Context, u64) -> R + Send + 'static,
) -> Duration
where
    R: std::future::Future<Output = Duration> + Send,
{
    use elfo::{config::AnyConfig, prelude::*, MoveOwnership, Topology};

    #[message(ret = Duration)]
    struct Start;

    let topology = Topology::empty();
    let testee = topology.local("testee");
    let configurers = topology.local("system.configurers").entrypoint();

    let testee_addr = testee.addr();

    let body = MoveOwnership::from(f);
    let blueprint = ActorGroup::new().exec(move |mut ctx| {
        let body = body.clone();

        async move {
            msg!(match ctx.recv().await.unwrap() {
                (Start, token) => {
                    let body = body.take().unwrap();
                    let pruned_ctx = ctx.pruned();
                    let spent = body(ctx, iter_count).await;
                    pruned_ctx.respond(token, spent);
                }
                _ => unreachable!(),
            });
        }
    });
    testee.mount(blueprint);

    configurers.mount(elfo::batteries::configurer::fixture(
        &topology,
        AnyConfig::default(),
    ));

    let rt = make_st_runtime();
    rt.block_on(elfo::_priv::do_start(
        topology,
        false,
        |ctx, _| async move { ctx.request_to(testee_addr, Start).resolve().await.unwrap() },
    ))
    .unwrap()
}
