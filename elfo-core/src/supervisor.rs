use std::{
    any::Any, future::Future, mem, ops::Deref, panic::AssertUnwindSafe, sync::Arc, time::Duration,
};

use dashmap::DashMap;
use futures::{future::BoxFuture, FutureExt};
use fxhash::FxBuildHasher;
use metrics::{decrement_gauge, increment_gauge};
use parking_lot::RwLock;
use tracing::{debug, error_span, info, warn, Instrument, Span};

use crate::{self as elfo};
use elfo_macros::msg_raw as msg;
use elfo_utils::CachePadded;

use self::{backoff::Backoff, error_chain::ErrorChain, measure_poll::MeasurePoll};
use crate::{
    actor::{Actor, ActorMeta, ActorStatus},
    addr::Addr,
    config::{AnyConfig, Config, SystemConfig},
    context::Context,
    envelope::Envelope,
    errors::TrySendError,
    exec::{Exec, ExecResult},
    group::{RestartMode, RestartPolicy, TerminationPolicy},
    messages,
    object::{Object, ObjectArc},
    routers::{Outcome, Router},
    runtime::RuntimeManager,
    scope::{self, Scope, ScopeGroupShared},
    subscription::SubscriptionManager,
    tracing::TraceId,
};

mod backoff;
mod error_chain;
mod measure_poll;

pub(crate) struct Supervisor<R: Router<C>, C, X> {
    meta: Arc<ActorMeta>,
    restart_policy: RestartPolicy,
    termination_policy: TerminationPolicy,
    span: Span,
    context: Context,
    objects: DashMap<R::Key, ObjectArc, FxBuildHasher>,
    router: R,
    exec: X,
    control: CachePadded<RwLock<ControlBlock<C>>>,
    scope_shared: Arc<ScopeGroupShared>,
    status_subscription: Arc<SubscriptionManager>,
    rt_manager: RuntimeManager,
}

struct ControlBlock<C> {
    system_config: Arc<SystemConfig>,
    user_config: Option<Arc<C>>,
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
                .or_try_insert_with(|| $this.spawn(key, Default::default()).ok_or(()))
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
        rt_manager: RuntimeManager,
    ) -> Self {
        let control = ControlBlock {
            system_config: Default::default(),
            user_config: None,
            is_started: false,
            stop_spawning: false,
        };

        let status_subscription = SubscriptionManager::new(ctx.clone());

        Self {
            span: error_span!(parent: Span::none(), "", actor_group = group.as_str()),
            meta: Arc::new(ActorMeta {
                group,
                key: String::new(),
            }),
            restart_policy,
            termination_policy,
            objects: DashMap::default(),
            router,
            exec,
            control: CachePadded(RwLock::new(control)),
            scope_shared: Arc::new(ScopeGroupShared::new(ctx.group())),
            status_subscription: Arc::new(status_subscription),
            context: ctx,
            rt_manager,
        }
    }

    // This method shouldn't be called often.
    fn in_scope(&self, f: impl FnOnce()) {
        Scope::new(
            scope::trace_id(),
            Addr::NULL,
            self.meta.clone(),
            // TODO: do not limit logging and dumping in supervisor.
            self.scope_shared.clone(),
        )
        .sync_within(|| self.span.in_scope(f));
    }

    pub(crate) fn handle(self: &Arc<Self>, envelope: Envelope) -> RouteReport {
        let sender = envelope.sender();

        msg!(match &envelope {
            messages::ValidateConfig { config } => match config.decode::<C>() {
                Ok(config) => {
                    // Make all updates under lock, including telemetry/dumper ones.
                    let mut control = self.control.write();

                    if control.user_config.is_none() {
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
                    if !only_spawn || control.user_config.is_none() {
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
            messages::SubscribeToActorStatuses => {
                self.in_scope(|| self.subscribe_to_statuses(sender));
                RouteReport::Done
            }
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
            messages::Ping => {
                let outcome = self.router.route(&envelope);
                self.do_handle(envelope, outcome.or(Outcome::Broadcast))
            }
            _ => {
                let outcome = self.router.route(&envelope);
                self.do_handle(envelope, outcome.or(Outcome::Discard))
            }
        })
    }

    fn do_handle(self: &Arc<Self>, envelope: Envelope, outcome: Outcome<R::Key>) -> RouteReport {
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
            Outcome::GentleUnicast(key) => {
                let object = ward!(self.objects.get(&key), return RouteReport::Closed(envelope));
                let actor = object.as_actor().expect("supervisor stores only actors");
                match actor.try_send(envelope) {
                    Ok(()) => RouteReport::Done,
                    Err(TrySendError::Full(envelope)) => RouteReport::Wait(object.addr(), envelope),
                    Err(TrySendError::Closed(envelope)) => RouteReport::Closed(envelope),
                }
            }
            Outcome::Multicast(list) => self.do_handle_multiple(
                envelope,
                list.into_iter().filter_map(|key| get_or_spawn!(self, key)),
            ),
            Outcome::GentleMulticast(list) => self.do_handle_multiple(
                envelope,
                list.into_iter().filter_map(|key| self.objects.get(&key)),
            ),
            Outcome::Broadcast => self.do_handle_multiple(envelope, self.objects.iter()),
            Outcome::Discard => RouteReport::Closed(envelope),
            Outcome::Default => unreachable!("must be altered earlier"),
        }
    }

    fn do_handle_multiple(
        &self,
        envelope: Envelope,
        mut iter: impl Iterator<Item = impl Deref<Target = ObjectArc>>,
    ) -> RouteReport {
        let mut waiters = Vec::new();
        let mut someone = false;

        for object in &mut iter {
            // TODO: we shouldn't clone `envelope` for the last object in a sequence.
            // If a requester has died, go out.
            let envelope = ward!(envelope.duplicate(self.context.book()), break);

            let actor = object.as_actor().expect("supervisor stores only actors");
            match actor.try_send(envelope) {
                Ok(_) => someone = true,
                Err(TrySendError::Full(envelope)) => waiters.push((object.addr(), envelope)),
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

    fn spawn(self: &Arc<Self>, key: R::Key, mut backoff: Backoff) -> Option<ObjectArc> {
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

        let system_config = control.system_config.clone();
        let user_config = control
            .user_config
            .as_ref()
            .cloned()
            .expect("config is unset");

        let ctx = self
            .context
            .clone()
            .with_addr(addr)
            .with_key(key.clone())
            .with_config(user_config);

        drop(control);

        let sv = self.clone();

        // TODO: move to `harness.rs`.
        let fut = async move {
            let thread = std::thread::current();

            info!(%addr, thread = %thread.name().unwrap_or("?"), "started");

            sv.objects
                .get(&key)
                .expect("where is the current actor?")
                .as_actor()
                .expect("a supervisor stores only actors")
                .on_start();

            let fut = AssertUnwindSafe(async { sv.exec.exec(ctx).await.unify() }).catch_unwind();
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
                let after = backoff.next();

                if after == Duration::from_secs(0) {
                    debug!("actor will be restarted immediately");
                } else {
                    debug!(?after, "actor will be restarted");

                    increment_gauge!("elfo_restarting_actors", 1.);
                    tokio::time::sleep(after).await;
                    decrement_gauge!("elfo_restarting_actors", 1.);
                }

                // Restarted actors should have a new trace id.
                scope::set_trace_id(TraceId::generate());

                backoff.start();
                if let Some(object) = sv.spawn(key.clone(), backoff) {
                    sv.objects.insert(key.clone(), object)
                } else {
                    sv.objects.remove(&key).map(|(_, v)| v)
                }
            } else {
                debug!("actor won't be restarted");
                sv.objects.remove(&key).map(|(_, v)| v)
            }
            .expect("where is the current actor?");

            sv.context.book().remove(addr);
        };

        let meta = Arc::new(ActorMeta {
            group: self.meta.group.clone(),
            key: key_str,
        });

        let rt = self.rt_manager.get(&meta);

        let actor = Actor::new(
            meta.clone(),
            addr,
            self.termination_policy.clone(),
            self.status_subscription.clone(),
        );
        entry.insert(Object::new(addr, actor));

        let scope = Scope::new(scope::trace_id(), addr, meta, self.scope_shared.clone())
            .with_telemetry(&system_config.telemetry);
        let fut = MeasurePoll::new(fut.instrument(span));

        rt.spawn(scope.within(fut));
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
            Outcome::GentleUnicast(_)
            | Outcome::GentleMulticast(_)
            | Outcome::Broadcast
            | Outcome::Discard
            | Outcome::Default => {}
        }
    }

    fn update_config(&self, control: &mut ControlBlock<C>, config: &AnyConfig) {
        let system = config.get_system();
        self.scope_shared.configure(system);

        // Update user's config.
        control.system_config = config.get_system().clone();
        control.user_config = Some(config.get_user::<C>().clone());
        self.router
            .update(control.user_config.as_ref().expect("just saved"));
        self.in_scope(|| info!(config = ?control.user_config.as_ref().unwrap(), "router updated"));
    }

    fn subscribe_to_statuses(&self, addr: Addr) {
        // Firstly, add the subscriber to handle new objects right way.
        self.status_subscription.add(addr);

        // Send active statuses to the subscriber.
        for item in self.objects.iter() {
            let actor = item
                .value()
                .as_actor()
                .expect("a supervisor stores only actors");

            let result = actor.with_status(|report| self.context.try_send_to(addr, report));

            if result.is_err() {
                // TODO: `unbounded_send(Unsubscribed)`
                warn!(%addr, "status cannot be sent, unsubscribing");
                self.status_subscription.remove(addr);
                break;
            }
        }
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
