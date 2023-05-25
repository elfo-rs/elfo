use std::{future::Future, sync::Arc, time::Duration};

use futures::future::join_all;
use tokio::{
    pin, select,
    time::{sleep, timeout, Instant},
};
use tracing::{error, info, level_filters::LevelFilter, warn};

use crate::{
    actor::{Actor, ActorMeta, ActorStatus},
    config::SystemConfig,
    context::Context,
    demux::Demux,
    errors::{RequestError, StartError, StartGroupError},
    memory_tracker::MemoryTracker,
    message,
    messages::{StartEntrypoint, Terminate, UpdateConfig},
    msg,
    object::Object,
    scope::{Scope, ScopeGroupShared},
    signal::{Signal, SignalKind},
    subscription::SubscriptionManager,
    time::Interval,
    topology::Topology,
    tracing::TraceId,
    Addr,
};

type Result<T, E = StartError> = std::result::Result<T, E>;

async fn start_entrypoints(ctx: &Context, topology: &Topology, is_check_only: bool) -> Result<()> {
    let futures = topology
        .locals()
        .filter(|group| group.is_entrypoint)
        .map(|group| async move {
            let response = ctx
                .request_to(
                    group.addr,
                    UpdateConfig {
                        config: Default::default(),
                    },
                )
                .resolve()
                .await;
            match response {
                Ok(Ok(())) => Ok(()),
                Ok(Err(e)) => Err(StartError::single(group.name.clone(), e.reason)),
                Err(RequestError::Ignored) => Ok(()),
                Err(RequestError::Closed(..)) => Err(StartError::single(
                    group.name.clone(),
                    "config cannot be delivered to the entrypoint".into(),
                )),
            }?;

            let response = ctx
                .request_to(group.addr, StartEntrypoint { is_check_only })
                .resolve()
                .await;
            match response {
                Ok(Ok(())) => Ok(()),
                Ok(Err(e)) => {
                    let group_errors: Vec<StartGroupError> = e
                        .errors
                        .into_iter()
                        .map(|e| StartGroupError {
                            group: e.group,
                            reason: e.reason,
                        })
                        .collect();
                    Err(StartError::multiple(group_errors))
                }
                Err(RequestError::Ignored) => Ok(()),
                Err(RequestError::Closed(..)) => Err(StartError::single(
                    group.name,
                    "starting message cannot be delivered to the entrypoint".into(),
                )),
            }
        });

    let errors: Vec<StartGroupError> = futures::future::join_all(futures)
        .await
        .into_iter()
        .filter_map(Result::err)
        .flat_map(|e| e.errors.into_iter())
        .collect();

    if errors.is_empty() {
        Ok(())
    } else {
        Err(StartError::multiple(errors))
    }
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
    let res = do_start(topology, false, termination).await;

    if res.is_err() {
        // XXX: give enough time to the logger.
        sleep(Duration::from_millis(500)).await;
    }

    res
}

/// Starts node in "check only" mode. Entrypoints are started, then the system
/// is immediately gracefully terminated.
pub async fn check_only(topology: Topology) -> Result<()> {
    // The logger is not supposed to be initialized in this mode, so we do not wait
    // for it before exiting.
    do_start(topology, true, do_termination).await
}

#[doc(hidden)]
pub async fn do_start<F: Future>(
    topology: Topology,
    is_check_only: bool,
    and_then: impl FnOnce(Context, Topology) -> F,
) -> Result<F::Output> {
    message::init();

    let entry = topology.book.vacant_entry(0);
    let addr = entry.addr();
    let ctx = Context::new(topology.book.clone(), Demux::default());

    let meta = Arc::new(ActorMeta {
        group: "system.init".into(),
        key: "_".into(), // Just like `Singleton`.
    });

    // XXX: create a real group.
    let actor = Actor::new(
        meta.clone(),
        addr,
        Default::default(),
        Arc::new(SubscriptionManager::new(ctx.clone())),
    );

    let scope_shared = ScopeGroupShared::new(addr);
    let mut config = SystemConfig::default();
    config.logging.max_level = LevelFilter::INFO;
    scope_shared.configure(&config);

    let scope = Scope::new(TraceId::generate(), addr, meta, Arc::new(scope_shared));
    scope.clone().sync_within(|| actor.on_start()); // need to emit initial metrics
    entry.insert(Object::new(addr, actor));

    // It must be called after `entry.insert()`.
    let ctx = ctx.with_addr(addr);

    let init = async move {
        start_entrypoints(&ctx, &topology, is_check_only).await?;
        Ok(and_then(ctx, topology).await)
    };
    scope.within(init).await
}

#[message]
struct TerminateSystem;

#[message]
struct CheckMemoryUsageTick;

// TODO: make these values configurable.
const SEND_CLOSING_TERMINATE_AFTER: Duration = Duration::from_secs(30);
const STOP_GROUP_TERMINATION_AFTER: Duration = Duration::from_secs(45);
const MAX_MEMORY_USAGE_RATIO: f64 = 0.9;
const CHECK_MEMORY_USAGE_INTERVAL: Duration = Duration::from_secs(3);

async fn termination(mut ctx: Context, topology: Topology) {
    ctx.attach(Signal::new(SignalKind::UnixTerminate, TerminateSystem));
    ctx.attach(Signal::new(SignalKind::UnixInterrupt, TerminateSystem));
    ctx.attach(Signal::new(SignalKind::WindowsCtrlC, TerminateSystem));

    let memory_tracker = match MemoryTracker::new(MAX_MEMORY_USAGE_RATIO) {
        Ok(tracker) => {
            ctx.attach(Interval::new(CheckMemoryUsageTick))
                .start(CHECK_MEMORY_USAGE_INTERVAL);
            Some(tracker)
        }
        Err(err) => {
            warn!(error = %err, "memory tracker is unavailable, disabled");
            None
        }
    };

    let mut oom_prevented = false;

    while let Some(envelope) = ctx.recv().await {
        msg!(match envelope {
            TerminateSystem => break, // TODO: use `Terminate`?
            CheckMemoryUsageTick => {
                match memory_tracker.as_ref().map(|mt| mt.check()) {
                    Some(Ok(true)) | None => {}
                    Some(Ok(false)) => {
                        error!("maximum memory usage is reached, forcibly terminating");
                        let _ = ctx.try_send_to(ctx.addr(), TerminateSystem);
                        oom_prevented = true;
                    }
                    Some(Err(err)) => {
                        warn!(error = %err, "memory tracker cannot check memory usage");
                    }
                }
            }
        });
    }

    ctx.set_status(ActorStatus::TERMINATING);

    let termination = do_termination(ctx.pruned(), topology);
    pin!(termination);

    loop {
        select! {
            _ = &mut termination => return,
            Some(envelope) = ctx.recv() => {
                if !envelope.is::<TerminateSystem>() {
                    continue;
                }

                if oom_prevented {
                    // Skip the first signal after OOM prevented.
                    oom_prevented = false;
                } else {
                    // `Ctrl-C` has been pressed again. Terminate immediately.
                    // TODO: `Terminate::closing` on second `Ctrl-C`
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
    // TODO: specify order of system groups.
    let futures = topology
        .locals()
        .filter(|group| user ^ group.name.starts_with("system."))
        .map(|group| async move {
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
