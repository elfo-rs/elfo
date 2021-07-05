use std::{
    future::{self, Future},
    sync::Arc,
};

use futures::TryFutureExt;

use crate::{
    actor::Actor,
    context::Context,
    demux::Demux,
    dumping::Filter as DumperFilter,
    errors::{RequestError, StartError},
    message,
    messages::{Ping, UpdateConfig},
    object::{Object, ObjectMeta},
    tls,
    topology::Topology,
    trace_id,
};

type Result<T, E = StartError> = std::result::Result<T, E>;

async fn send_configs_to_entrypoints(ctx: &Context, topology: &Topology) -> Result<()> {
    let futures = topology
        .actor_groups()
        .filter(|group| group.is_entrypoint)
        .map(|group| {
            let config = Default::default();
            ctx.request(UpdateConfig { config })
                .from(group.addr)
                .resolve()
                .or_else(|err| async move {
                    match err {
                        RequestError::Ignored => Ok(Ok(())),
                        _ => Err(StartError::Other(
                            "initial messages cannot be delivered".into(),
                        )),
                    }
                })
        });

    let error_count = futures::future::try_join_all(futures)
        .await?
        .into_iter()
        .filter_map(Result::err)
        .count();

    if error_count == 0 {
        Ok(())
    } else {
        Err(StartError::InvalidConfig)
    }
}

async fn start_entrypoints(ctx: &Context, topology: &Topology) -> Result<()> {
    let futures = topology
        .actor_groups()
        .filter(|group| group.is_entrypoint)
        .map(|group| ctx.request(Ping).from(group.addr).resolve());

    futures::future::try_join_all(futures)
        .await
        .map_err(|_| StartError::Other("entrypoint cannot started".into()))?;

    Ok(())
}

pub async fn start(topology: Topology) {
    try_start(topology).await.expect("cannot start")
}

pub async fn try_start(topology: Topology) -> Result<()> {
    do_start(topology, |_| future::ready(())).await?;

    // TODO: graceful termination based on topology.
    let () = future::pending().await;
    Ok(())
}

#[doc(hidden)]
pub async fn do_start<F: Future>(
    topology: Topology,
    f: impl FnOnce(Context) -> F,
) -> Result<F::Output> {
    message::init();

    let entry = topology.book.vacant_entry();
    let addr = entry.addr();
    entry.insert(Object::new(addr, Actor::new(addr)));

    let dumper = topology.dumper.for_group(DumperFilter::All);

    let meta = ObjectMeta {
        group: "starter".into(),
        key: Some("_".into()), // Just like `Singleton`.
    };
    let initial_trace_id = trace_id::generate();
    tls::scope(Arc::new(meta), initial_trace_id, async move {
        let ctx = Context::new(topology.book.clone(), dumper, Demux::default()).with_addr(addr);
        send_configs_to_entrypoints(&ctx, &topology).await?;
        start_entrypoints(&ctx, &topology).await?;
        Ok(f(ctx).await)
    })
    .await
}
