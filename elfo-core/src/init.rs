use std::{future::Future, sync::Arc, time::Duration};

use futures::future::join_all;
use tokio::{
    pin, select,
    time::{sleep, timeout},
};
use tracing::{error, info, level_filters::LevelFilter, warn};

use elfo_utils::time::Instant;

#[cfg(target_os = "linux")]
use crate::{memory_tracker::MemoryTracker, time::Interval};

use crate::{
    actor::{Actor, ActorMeta, ActorStatus},
    addr::{Addr, GroupNo},
    config::SystemConfig,
    context::Context,
    demux::Demux,
    errors::{RequestError, StartError, StartGroupError},
    message,
    messages::{StartEntrypoint, Terminate, UpdateConfig},
    object::Object,
    scope::{Scope, ScopeGroupShared},
    signal::{Signal, SignalKind},
    subscription::SubscriptionManager,
    topology::{Topology, SYSTEM_INIT_GROUP_NO},
    tracing::TraceId,
};

const INIT_GROUP_NAME: &str = "system.init";

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
                Err(RequestError::Failed) => Err(StartError::single(
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
                Err(RequestError::Failed) => Err(StartError::single(
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
    check_messages_uniqueness()?;

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
    check_messages_uniqueness()?;

    // The logger is not supposed to be initialized in this mode, so we do not wait
    // for it before exiting.
    do_start(topology, true, do_termination).await
}

/// Checks that all messages are unique by `(protocol, name)` pair.
/// If there are duplicates, returns an error.
///
/// It's called automatically by `(try_)start()` and `check_only()`,
/// but still provided in order to being called manually in service tests.
#[stability::unstable]
pub fn check_messages_uniqueness() -> Result<()> {
    message::check_uniqueness().map_err(|duplicates| {
        let errors = duplicates
            .into_iter()
            .map(|(protocol, name)| StartGroupError {
                group: INIT_GROUP_NAME.into(),
                reason: format!("message `{}/{}` is defined several times", protocol, name),
            })
            .collect();

        StartError::multiple(errors)
    })
}

#[doc(hidden)]
pub async fn do_start<F: Future>(
    topology: Topology,
    is_check_only: bool,
    and_then: impl FnOnce(Context, Topology) -> F,
) -> Result<F::Output> {
    // Perform the clock calibration if needed.
    Instant::now();

    let group_no = GroupNo::new(SYSTEM_INIT_GROUP_NO, topology.launch_id()).unwrap();
    let entry = topology.book.vacant_entry(group_no);
    let addr = entry.addr();
    let ctx = Context::new(topology.book.clone(), Demux::default());

    let meta = Arc::new(ActorMeta {
        group: INIT_GROUP_NAME.into(),
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
const SEND_CLOSING_TERMINATE_AFTER: Duration = Duration::from_secs(25);
const STOP_GROUP_TERMINATION_AFTER: Duration = Duration::from_secs(35);

async fn termination(mut ctx: Context, topology: Topology) {
    ctx.attach(Signal::new(SignalKind::UnixTerminate, TerminateSystem));
    ctx.attach(Signal::new(SignalKind::UnixInterrupt, TerminateSystem));
    ctx.attach(Signal::new(SignalKind::WindowsCtrlC, TerminateSystem));

    #[cfg(target_os = "linux")]
    let memory_tracker = {
        const MAX_MEMORY_USAGE_RATIO: f64 = 0.9;
        const CHECK_MEMORY_USAGE_INTERVAL: Duration = Duration::from_secs(3);

        match MemoryTracker::new(MAX_MEMORY_USAGE_RATIO) {
            Ok(tracker) => {
                ctx.attach(Interval::new(CheckMemoryUsageTick))
                    .start(CHECK_MEMORY_USAGE_INTERVAL);
                Some(tracker)
            }
            Err(err) => {
                warn!(error = %err, "memory tracker is unavailable, disabled");
                None
            }
        }
    };

    let mut oom_prevented = false;

    while let Some(envelope) = ctx.recv().await {
        if envelope.is::<TerminateSystem>() {
            break;
        }

        #[cfg(target_os = "linux")]
        if envelope.is::<CheckMemoryUsageTick>() {
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
    let mut stop_order_list = topology
        .locals()
        .map(|group| group.stop_order)
        .collect::<Vec<_>>();

    stop_order_list.sort_unstable();
    stop_order_list.dedup();

    for stop_order in stop_order_list {
        info!(%stop_order, "terminating groups");
        terminate_groups(&ctx, &topology, stop_order).await;
    }
}

async fn terminate_groups(ctx: &Context, topology: &Topology, stop_order: i8) {
    let futures = topology
        .locals()
        .filter(|group| group.stop_order == stop_order)
        .map(|group| async move {
            let started_at = Instant::now();
            select! {
                _ = terminate_group(ctx, group.addr, group.name.clone(), started_at) => {},
                _ = watch_group(ctx, group.addr, group.name, started_at) => {},
            }
        })
        .collect::<Vec<_>>();

    join_all(futures).await;
}

async fn terminate_group(ctx: &Context, addr: Addr, name: String, started_at: Instant) {
    // Terminate::default

    info!(group = %name, "sending polite Terminate");
    let fut = ctx.send_to(addr, Terminate::default());

    if timeout(SEND_CLOSING_TERMINATE_AFTER, fut).await.is_ok() {
        let elapsed = started_at.elapsed();
        if let Some(delta) = SEND_CLOSING_TERMINATE_AFTER.checked_sub(elapsed) {
            sleep(delta).await;
        }
    } else {
        warn!(group = %name, "failed to deliver polite Terminate, some actors are too busy");
    }

    // Terminate::closing

    warn!(
        message = "actor group hasn't finished yet, sending closing Terminate",
        group = %name,
        elapsed = ?started_at.elapsed(),
    );
    let fut = ctx.send_to(addr, Terminate::closing());

    if timeout(STOP_GROUP_TERMINATION_AFTER, fut).await.is_ok() {
        let elapsed = started_at.elapsed();
        if let Some(delta) = STOP_GROUP_TERMINATION_AFTER.checked_sub(elapsed) {
            sleep(delta).await;
        }
    } else {
        warn!(group = %name, "failed to deliver closing Terminate");
    }

    error!(
        message = "failed to terminate an actor group, skipped",
        group = %name,
        elapsed = ?started_at.elapsed(),
    );
}

async fn watch_group(ctx: &Context, addr: Addr, name: String, started_at: Instant) {
    ctx.finished(addr).await;

    info!(
        message = "actor group finished",
        group = %name,
        elapsed = ?started_at.elapsed(),
    );
}
