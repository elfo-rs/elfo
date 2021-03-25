use crate::{
    context::Context, demux::Demux, errors::StartError, messages, object::Object,
    topology::Topology,
};

type Result<T, E = StartError> = std::result::Result<T, E>;

async fn send_configs(ctx: &Context, topology: &Topology) -> Result<()> {
    let futures = topology
        .actor_groups()
        .filter(|group| group.is_entrypoint)
        .map(|group| {
            let config = Default::default();
            ctx.request(messages::UpdateConfig { config })
                .from(group.addr)
                .resolve()
        });

    let error_count = futures::future::try_join_all(futures)
        .await
        .map_err(|_| StartError::Other("initial messages cannot be delivered".into()))?
        .into_iter()
        .filter_map(Result::err)
        .count();

    if error_count == 0 {
        Ok(())
    } else {
        Err(StartError::InvalidConfig)
    }
}

pub async fn start(topology: Topology) {
    try_start(topology).await.expect("cannot start")
}

pub async fn try_start(topology: Topology) -> Result<()> {
    let entry = topology.book.vacant_entry();
    let addr = entry.addr();
    entry.insert(Object::new_actor(addr));

    let ctx = Context::new(topology.book.clone(), Demux::default()).with_addr(addr);

    send_configs(&ctx, &topology).await?;

    // TODO: graceful termination based on topology.
    let () = futures::future::pending().await;
    Ok(())
}
