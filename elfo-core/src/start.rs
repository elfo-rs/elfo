use std::{future::Future, sync::Arc, time::Duration};

use futures::{future::join_all, TryFutureExt};
use tokio::{
    pin, select,
    time::{sleep, timeout, Instant},
};
use tracing::{error, info, warn, Level};

use elfo_macros::message;

use crate::{
    actor::Actor,
    addr::Addr,
    context::Context,
    demux::Demux,
    errors::{RequestError, StartError},
    message,
    messages::{Ping, Terminate, UpdateConfig},
    object::{Object, ObjectMeta},
    permissions::{AtomicPermissions, Permissions},
    scope::Scope,
    signal::{Signal, SignalKind},
    topology::Topology,
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

/// Starts a node with the provided topology.
///
/// # Panics
///
/// Panics if the system cannot initialize.
/// Usually, it happens because of an invalid config.
pub async fn start(topology: Topology) {
    try_start(topology).await.expect("cannot start")
}

/// The same as `start()`, but returns an error rather than panics.
pub async fn try_start(topology: Topology) -> Result<()> {
    do_start(topology, termination).await?;
    Ok(())
}

#[doc(hidden)]
pub async fn do_start<F: Future>(
    topology: Topology,
    f: impl FnOnce(Context, Topology) -> F,
) -> Result<F::Output> {
    message::init();

    let entry = topology.book.vacant_entry();
    let addr = entry.addr();
    entry.insert(Object::new(addr, Actor::new(addr, Default::default())));

    let dumper = topology.dumper.for_group(false); // TODO: should we dump the starter?

    let meta = ObjectMeta {
        group: "init".into(),
        key: Some("_".into()), // Just like `Singleton`.
    };

    // XXX: `addr` is used for both a specific actor and a whole group for now.
    let perm = Arc::new(AtomicPermissions::default());
    perm.store({
        let mut perm = Permissions::default();
        perm.set_logging_enabled(Some(Level::INFO));
        perm
    });

    let scope = Scope::new(addr, addr, Arc::new(meta), perm, Default::default());
    let f = async move {
        let ctx = Context::new(topology.book.clone(), dumper, Demux::default()).with_addr(addr);
        send_configs_to_entrypoints(&ctx, &topology).await?;
        start_entrypoints(&ctx, &topology).await?;
        Ok(f(ctx, topology).await)
    };
    scope.within(f).await
}

#[message(elfo = crate)]
struct TerminateSystem;

const SEND_CLOSING_TERMINATE_AFTER: Duration = Duration::from_secs(30);
const STOP_GROUP_TERMINATION_AFTER: Duration = Duration::from_secs(45);

async fn termination(ctx: Context, topology: Topology) {
    let term = Signal::new(SignalKind::Terminate, || TerminateSystem);
    let ctrl_c = Signal::new(SignalKind::CtrlC, || TerminateSystem);

    let mut ctx = ctx.with(&term).with(&ctrl_c);

    while let Some(envelope) = ctx.recv().await {
        if envelope.is::<TerminateSystem>() {
            break;
        }
    }

    let termination = do_termination(ctx.pruned(), topology);
    pin!(termination);

    loop {
        select! {
            _ = &mut termination => return,
            Some(envelope) = ctx.recv() => {
                // `Ctrl-C` has been pressed again.
                // Terminate immediately.
                if envelope.is::<TerminateSystem>() {
                    return;
                }
            }
        }
    }
}

async fn do_termination(ctx: Context, topology: Topology) {
    info!("terminating user actor groups");
    terminate_groups(&ctx, &topology, true).await;
    info!("terminating system actor groups");
    terminate_groups(&ctx, &topology, false).await;
    info!("system terminated");
}

async fn terminate_groups(ctx: &Context, topology: &Topology, user: bool) {
    let futures = topology
        .actor_groups()
        .filter(|group| user ^ group.name.starts_with("system."))
        .map(|group| async {
            select! {
                _ = terminate_group(ctx, group.addr, group.name.clone()) => {},
                _ = watch_group(ctx, group.addr, group.name) => {},
            }
        })
        .collect::<Vec<_>>();

    join_all(futures).await;
}

async fn terminate_group(ctx: &Context, addr: Addr, name: String) {
    let start_time = Instant::now();

    // Terminate::default

    info!(group = %name, "sending polite Terminate");
    let fut = ctx.send_to(addr, Terminate::default());

    if timeout(SEND_CLOSING_TERMINATE_AFTER, fut).await.is_ok() {
        let elapsed = start_time.elapsed();
        if let Some(delta) = SEND_CLOSING_TERMINATE_AFTER.checked_sub(elapsed) {
            sleep(delta).await;
        }
    } else {
        warn!(
            group = %name,
            "failed to deliver polite Terminate, some actors are too busy"
        );
    }

    // Terminate::closing

    warn!(
        group = %name,
        "actor group hasn't finished yet, sending closing terminate"
    );
    let fut = ctx.send_to(addr, Terminate::closing());

    if timeout(STOP_GROUP_TERMINATION_AFTER, fut).await.is_ok() {
        let elapsed = start_time.elapsed();
        if let Some(delta) = STOP_GROUP_TERMINATION_AFTER.checked_sub(elapsed) {
            sleep(delta).await;
        }
    } else {
        warn!(
            group = %name,
            "failed to deliver closing Terminate"
        );
    }

    error!(group = %name, "failed to terminate an actor group");
}

async fn watch_group(ctx: &Context, addr: Addr, name: String) {
    ctx.finished(addr).await;
    info!(group = %name, "actor group finished");
}
