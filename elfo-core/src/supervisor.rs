use std::{any::Any, future::Future, mem, panic::AssertUnwindSafe, sync::Arc, time::Duration};

use dashmap::DashMap;
use futures::{future::BoxFuture, FutureExt};
use fxhash::FxBuildHasher;
use metrics::{decrement_gauge, increment_gauge};
use parking_lot::RwLock;
use tracing::{error_span, info, Instrument, Span};

use crate as elfo;
use elfo_macros::msg_raw as msg;
use elfo_utils::{CachePadded, ErrorChain, RateLimiter};

use crate::{
    actor::{Actor, ActorStatus},
    addr::Addr,
    config::{AnyConfig, Config},
    context::Context,
    envelope::Envelope,
    errors::TrySendError,
    exec::{Exec, ExecResult},
    group::{RestartMode, RestartPolicy, TerminationPolicy},
    messages,
    object::{Object, ObjectArc, ObjectMeta},
    permissions::AtomicPermissions,
    routers::{Outcome, Router},
    scope::{self, Scope},
};

pub(crate) struct Supervisor<R: Router<C>, C, X> {
    meta: Arc<ObjectMeta>,
    restart_policy: RestartPolicy,
    termination_policy: TerminationPolicy,
    span: Span,
    context: Context,
    // TODO: replace with `crossbeam_utils::sync::ShardedLock`?
    objects: DashMap<R::Key, ObjectArc, FxBuildHasher>,
    router: R,
    exec: X,
    control: CachePadded<RwLock<ControlBlock<C>>>,
    permissions: Arc<AtomicPermissions>,
    logging_limiter: Arc<RateLimiter>,
}

struct ControlBlock<C> {
    config: Option<Arc<C>>,
    is_started: bool,
    stop_spawning: bool,
}

/// Returns `None` if cannot be spawned.
macro_rules! get_or_spawn {
    ($this:ident, $key:expr) => {{
        let key = $key;
        match $this.objects.get(&key) {
            Some(object) => Some(object),
            None => $this
                .objects
                .entry(key.clone())
                .or_try_insert_with(|| $this.spawn(key).ok_or(()))
                .map(|o| o.downgrade())
                .ok(),
        }
    }};
}

impl<R, C, X> Supervisor<R, C, X>
where
    R: Router<C>,
    X: Exec<Context<C, R::Key>>,
    <X::Output as Future>::Output: ExecResult,
    C: Config,
{
    pub(crate) fn new(
        ctx: Context,
        group: String,
        exec: X,
        router: R,
        restart_policy: RestartPolicy,
        termination_policy: TerminationPolicy,
    ) -> Self {
        let control = ControlBlock {
            config: None,
            is_started: false,
            stop_spawning: false,
        };

        Self {
            span: error_span!(parent: Span::none(), "", actor_group = group.as_str()),
            meta: Arc::new(ObjectMeta { group, key: None }),
            restart_policy,
            termination_policy,
            context: ctx,
            objects: DashMap::default(),
            router,
            exec,
            control: CachePadded(RwLock::new(control)),
            permissions: Default::default(),
            logging_limiter: Default::default(),
        }
    }

    // This method shouldn't be called often.
    fn in_scope(&self, f: impl FnOnce()) {
        Scope::with_trace_id(
            scope::trace_id(),
            Addr::NULL,
            self.context.group(),
            self.meta.clone(),
            self.permissions.clone(),
            Default::default(), // Do not limit logging in supervisor.
        )
        .sync_within(|| self.span.in_scope(f));
    }

    pub(crate) fn handle(self: &Arc<Self>, envelope: Envelope) -> RouteReport {
        msg!(match &envelope {
            messages::ValidateConfig { config } => match config.decode::<C>() {
                Ok(config) => {
                    // Make all updates under lock, including telemetry/dumper ones.
                    let mut control = self.control.write();

                    if control.config.is_none() {
                        // We should update configs before spawning any actors
                        // to avoid a race condition at startup.
                        // So, we update the config on `ValidateConfig` at the first time.
                        self.update_config(&mut control, &config);
                        RouteReport::Done
                    } else {
                        drop(control);
                        let outcome = self.router.route(&envelope);
                        let mut envelope = envelope;
                        envelope.set_message(messages::ValidateConfig { config });
                        self.do_handle(envelope, outcome.or(Outcome::Broadcast))
                    }
                }
                Err(reason) => {
                    msg!(match envelope {
                        (messages::ValidateConfig { .. }, token) => {
                            let reject = messages::ConfigRejected { reason };
                            self.context.respond(token, Err(reject));
                        }
                        _ => unreachable!(),
                    });
                    RouteReport::Done
                }
            },
            messages::UpdateConfig { config } => match config.decode::<C>() {
                Ok(config) => {
                    // Make all updates under lock, including telemetry/dumper ones.
                    let mut control = self.control.write();

                    let only_spawn = !control.is_started;
                    if !only_spawn || control.config.is_none() {
                        // At the first time the config is updated on `ValidateConfig`.
                        self.update_config(&mut control, &config);
                    }

                    control.is_started = true;
                    drop(control);

                    let outcome = self.router.route(&envelope);

                    if only_spawn {
                        self.spawn_by_outcome(outcome);
                        RouteReport::Done
                    } else {
                        // Send `UpdateConfig` across actors.
                        let mut envelope = envelope;
                        envelope.set_message(messages::UpdateConfig { config });
                        self.do_handle(envelope, outcome.or(Outcome::Broadcast))
                    }
                }
                Err(reason) => {
                    msg!(match envelope {
                        (messages::UpdateConfig { .. }, token) => {
                            let reject = messages::ConfigRejected { reason };
                            self.context.respond(token, Err(reject));
                        }
                        _ => unreachable!(),
                    });
                    RouteReport::Done
                }
            },
            messages::Terminate => {
                if self.termination_policy.stop_spawning {
                    let is_newly = !mem::replace(&mut self.control.write().stop_spawning, true);
                    if is_newly {
                        self.in_scope(|| info!("stopped spawning new actors"));
                    }
                }

                let outcome = self.router.route(&envelope);
                self.do_handle(envelope, outcome.or(Outcome::Broadcast))
            }
            _ => {
                let outcome = self.router.route(&envelope);
                self.do_handle(envelope, outcome)
            }
        })
    }

    fn do_handle(self: &Arc<Self>, envelope: Envelope, outcome: Outcome<R::Key>) -> RouteReport {
        // TODO: avoid copy & paste.
        match outcome {
            Outcome::Unicast(key) => {
                let object = ward!(
                    get_or_spawn!(self, key),
                    return RouteReport::Closed(envelope)
                );
                let actor = object.as_actor().expect("supervisor stores only actors");
                match actor.try_send(envelope) {
                    Ok(()) => RouteReport::Done,
                    Err(TrySendError::Full(envelope)) => RouteReport::Wait(object.addr(), envelope),
                    Err(TrySendError::Closed(envelope)) => RouteReport::Closed(envelope),
                }
            }
            Outcome::Multicast(list) => {
                let mut waiters = Vec::new();
                let mut someone = false;

                // TODO: avoid the loop in `try_send` case.
                for key in list {
                    let object = ward!(get_or_spawn!(self, key), continue);

                    // TODO: we shouldn't clone `envelope` for the last object in a sequence.
                    let envelope = ward!(
                        envelope.duplicate(self.context.book()),
                        continue // A requester has died, but `Multicast` is more insistent.
                    );

                    let actor = object.as_actor().expect("supervisor stores only actors");

                    match actor.try_send(envelope) {
                        Ok(_) => someone = true,
                        Err(TrySendError::Full(envelope)) => {
                            waiters.push((object.addr(), envelope))
                        }
                        Err(TrySendError::Closed(_)) => {}
                    }
                }

                if waiters.is_empty() {
                    if someone {
                        RouteReport::Done
                    } else {
                        RouteReport::Closed(envelope)
                    }
                } else {
                    RouteReport::WaitAll(someone, waiters)
                }
            }
            Outcome::Broadcast => {
                let mut waiters = Vec::new();
                let mut someone = false;

                // TODO: avoid the loop in `try_send` case.
                for object in self.objects.iter() {
                    // TODO: we shouldn't clone `envelope` for the last object in a sequence.
                    let envelope = ward!(
                        envelope.duplicate(self.context.book()),
                        return RouteReport::Done // A requester has died.
                    );

                    let actor = object.as_actor().expect("supervisor stores only actors");

                    match actor.try_send(envelope) {
                        Ok(_) => someone = true,
                        Err(TrySendError::Full(envelope)) => {
                            waiters.push((object.addr(), envelope))
                        }
                        Err(TrySendError::Closed(_)) => {}
                    }
                }

                if waiters.is_empty() {
                    if someone {
                        RouteReport::Done
                    } else {
                        RouteReport::Closed(envelope)
                    }
                } else {
                    RouteReport::WaitAll(someone, waiters)
                }
            }
            Outcome::Discard | Outcome::Default => RouteReport::Done,
        }
    }

    fn spawn(self: &Arc<Self>, key: R::Key) -> Option<ObjectArc> {
        let control = self.control.read();
        if control.stop_spawning {
            return None;
        }

        let entry = self.context.book().vacant_entry();
        let addr = entry.addr();

        let key_str = key.to_string();
        let span = error_span!(
            parent: Span::none(),
            "",
            actor_group = self.meta.group.as_str(),
            actor_key = key_str.as_str()
        );

        let config = control.config.as_ref().cloned().expect("config is unset");

        let ctx = self
            .context
            .clone()
            .with_addr(addr)
            .with_key(key.clone())
            .with_config(config);

        drop(control);

        let sv = self.clone();

        // TODO: protect against panics (for `fn(..) -> impl Future`).
        let fut = self.exec.exec(ctx);

        // TODO: move to `harness.rs`.
        let fut = async move {
            info!(%addr, "started");

            sv.objects
                .get(&key)
                .expect("where is the current actor?")
                .as_actor()
                .expect("a supervisor stores only actors")
                .on_start();

            let fut = AssertUnwindSafe(async { fut.await.unify() }).catch_unwind();
            let new_status = match fut.await {
                Ok(Ok(())) => ActorStatus::TERMINATED,
                Ok(Err(err)) => ActorStatus::FAILED.with_details(ErrorChain(&*err)),
                Err(panic) => ActorStatus::FAILED.with_details(panic_to_string(panic)),
            };

            let should_restart = match sv.restart_policy.mode {
                RestartMode::Always => true,
                RestartMode::OnFailures => new_status.is_failed(),
                RestartMode::Never => false,
            };

            let need_to_restart = should_restart && !sv.control.read().stop_spawning;

            sv.objects
                .get(&key)
                .expect("where is the current actor?")
                .as_actor()
                .expect("a supervisor stores only actors")
                .set_status(new_status);

            if need_to_restart {
                increment_gauge!("elfo_restarting_actors", 1.);
                // TODO: use `backoff`.
                tokio::time::sleep(Duration::from_secs(5)).await;
                decrement_gauge!("elfo_restarting_actors", 1.);

                if let Some(object) = sv.spawn(key.clone()) {
                    sv.objects.insert(key.clone(), object)
                } else {
                    sv.objects.remove(&key).map(|(_, v)| v)
                }
            } else {
                sv.objects.remove(&key).map(|(_, v)| v)
            }
            .expect("where is the current actor?");

            sv.context.book().remove(addr);
        };

        let actor = Actor::new(addr, self.termination_policy.clone());
        entry.insert(Object::new(addr, actor));

        let meta = Arc::new(ObjectMeta {
            group: self.meta.group.clone(),
            key: Some(key_str),
        });

        let scope = Scope::new(
            addr,
            self.context.group(),
            meta,
            self.permissions.clone(),
            self.logging_limiter.clone(),
        );
        tokio::spawn(scope.within(fut.instrument(span)));
        let object = self.context.book().get_owned(addr).expect("just created");
        Some(object)
    }

    fn spawn_by_outcome(self: &Arc<Self>, outcome: Outcome<R::Key>) {
        match outcome {
            Outcome::Unicast(key) => {
                get_or_spawn!(self, key);
            }
            Outcome::Multicast(keys) => {
                for key in keys {
                    get_or_spawn!(self, key);
                }
            }
            Outcome::Broadcast | Outcome::Discard | Outcome::Default => {}
        }
    }

    fn update_config(&self, control: &mut ControlBlock<C>, config: &AnyConfig) {
        let system = config.get_system();

        // Update the dumper's config.
        self.context.dumper().configure(&system.dumping);

        // Update permissions.
        let mut perm = self.permissions.load();
        perm.set_logging_enabled(system.logging.max_level.to_tracing_level());
        perm.set_telemetry_per_actor_group_enabled(system.telemetry.per_actor_group);
        perm.set_telemetry_per_actor_key_enabled(system.telemetry.per_actor_key);
        self.permissions.store(perm);

        self.logging_limiter.configure(system.logging.max_rate);

        // Update user's config.
        control.config = Some(config.get_user::<C>().clone());
        self.router
            .update(control.config.as_ref().expect("just saved"));
        self.in_scope(|| info!(config = ?control.config.as_ref().unwrap(), "router updated"));
    }

    pub(crate) fn finished(self: &Arc<Self>) -> BoxFuture<'static, ()> {
        let sv = self.clone();
        let addrs = self
            .objects
            .iter()
            .map(|r| r.value().addr())
            .collect::<Vec<_>>();

        let fut = async move {
            for addr in addrs {
                let object = ward!(sv.context.book().get_owned(addr), continue);
                let actor = object.as_actor().expect("a supervisor stores only actors");
                actor.finished().await;
            }
        };

        Box::pin(fut)
    }
}

fn panic_to_string(payload: Box<dyn Any>) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        format!("panic: {}", message)
    } else if let Some(message) = payload.downcast_ref::<String>() {
        format!("panic: {}", message)
    } else {
        "panic: <unsupported payload>".to_string()
    }
}

pub(crate) enum RouteReport {
    Done,
    Closed(Envelope),
    Wait(Addr, Envelope),
    WaitAll(bool, Vec<(Addr, Envelope)>),
}
