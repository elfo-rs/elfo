use std::time::{Duration, Instant};

use criterion::{criterion_group, criterion_main, Criterion};

use elfo::{config::AnyConfig, prelude::*, topology::Topology};

mod common;

#[message(ret = Duration)]
struct Summarize;

fn make_yielder(iter_count: u32) -> Blueprint {
    ActorGroup::new().exec(move |mut ctx| async move {
        let token = msg!(match ctx.recv().await.unwrap() {
            (Summarize, token) => token,
            _ => unreachable!(),
        });

        let start = Instant::now();
        for _ in 0..iter_count {
            elfo::coop::consume_budget().await;
        }

        ctx.respond(token, start.elapsed());
    })
}

async fn run(iter_count: u32) -> Duration {
    let topology = Topology::empty();
    let yielder = topology.local("yielder");
    let configurers = topology.local("system.configurers").entrypoint();

    let yielder_addr = yielder.addr();

    yielder.mount(make_yielder(iter_count));
    configurers.mount(elfo::batteries::configurer::fixture(
        &topology,
        AnyConfig::default(),
    ));

    elfo::_priv::do_start(topology, false, |ctx, _| async move {
        ctx.request_to(yielder_addr, Summarize)
            .resolve()
            .await
            .unwrap()
    })
    .await
    .unwrap()
}

fn by_count(c: &mut Criterion) {
    c.bench_function("count", |b| {
        b.iter_custom(|iter_count| {
            let rt = common::make_mt_runtime(common::tokio_worker_threads());
            rt.block_on(run(iter_count as u32))
        })
    });
}

criterion_group!(cases, by_count);
criterion_main!(cases);
