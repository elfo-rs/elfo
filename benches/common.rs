use std::{env, str::FromStr, thread::available_parallelism};

use tokio::runtime::{Builder, Runtime};

#[allow(dead_code)]
pub(crate) fn tokio_worker_threads() -> u32 {
    env::var("TOKIO_WORKER_THREADS")
        .ok()
        .map(|s| u32::from_str(&s).expect("invalid value for TOKIO_WORKER_THREADS"))
        .unwrap_or_else(|| {
            usize::from(available_parallelism().expect("cannot get available parallelism")) as u32
        })
}

#[allow(dead_code)]
pub(crate) fn max_parallelism() -> u32 {
    env::var("ELFO_BENCH_MAX_PARALLELISM")
        .ok()
        .map(|s| u32::from_str(&s).expect("invalid value for ELFO_BENCH_MAX_PARALLELISM"))
        .unwrap_or_else(|| {
            usize::from(available_parallelism().expect("cannot get available parallelism")) as u32
        })
}

#[allow(dead_code)]
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
