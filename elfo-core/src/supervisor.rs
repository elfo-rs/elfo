use std::{any::Any, future::Future, mem, panic::AssertUnwindSafe, sync::Arc, time::Duration};

use futures::{future::BoxFuture, FutureExt};
use fxhash::FxBuildHasher;
use metrics::{decrement_gauge, increment_gauge};
use parking_lot::RwLock;
use scc::{
    ebr::Guard as EbrGuard,
    hash_index::{Entry as HashIndexEntry, HashIndex},
};
use tracing::{debug, error, error_span, info, warn, Instrument, Span};

use elfo_utils::CachePadded;

use self::{error_chain::ErrorChain, measure_poll::MeasurePoll};
use crate::{
    actor::{Actor, ActorMeta, ActorStartInfo, ActorStatus},
    config::{AnyConfig, Config, SystemConfig},
    context::Context,
    envelope::Envelope,
    exec::{Exec, ExecResult},
    group::TerminationPolicy,
    message::Request,
    messages, msg,
    object::{GroupVisitor, Object, ObjectRef},
    restarting::{RestartBackoff, RestartPolicy},
    routers::{Outcome, Router},
    runtime::RuntimeManager,
    scope::{self, Scope, ScopeGroupShared},
    subscription::SubscriptionManager,
    tracing::TraceId,
    Addr, ResponseToken,
};

mod error_chain;
mod measure_poll;

pub(crate) struct Supervisor<R: Router<C>, C, X> {
    meta: Arc<ActorMeta>,
    restart_policy: RestartPolicy,
    termination_policy: TerminationPolicy,
    span: Span,
    context: Context,
    objects: HashIndex<R::Key, Addr, FxBuildHasher>,
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
            objects: HashIndex::default(),
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

    pub(crate) fn handle(self: &Arc<Self>, mut envelope: Envelope, visitor: &mut dyn GroupVisitor) {
        let outcome = msg!(match &envelope {
            messages::ValidateConfig { config } => match config.decode::<C>() {
                Ok(config) => {
                    // Make all updates under lock, including telemetry/dumper ones.
                    let mut control = self.control.write();

                    if control.user_config.is_none() {
                        // We should update configs before spawning any actors
                        // to avoid a race condition at startup.
                        // So, we update the config on `ValidateConfig` at the first time.
                        self.update_config(&mut control, &config);
                        let token = extract_response_token::<messages::ValidateConfig>(envelope);
                        self.context.respond(token, Ok(()));
                        return visitor.done();
                    } else {
                        drop(control);
                        envelope.set_message(messages::ValidateConfig { config });
                        self.router.route(&envelope).or(Outcome::Discard)
                    }
                }
                Err(reason) => {
                    let reject = messages::ConfigRejected { reason };
                    let token = extract_response_token::<messages::ValidateConfig>(envelope);
                    self.context.respond(token, Err(reject));
                    return visitor.done();
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
                        self.spawn_on_group_mounted(outcome);
                        let token = extract_response_token::<messages::UpdateConfig>(envelope);
                        self.context.respond(token, Ok(()));
                        return visitor.done();
                    } else {
                        // Send `UpdateConfig` across actors.
                        envelope.set_message(messages::UpdateConfig { config });
                        outcome.or(Outcome::Broadcast)
                    }
                }
                Err(reason) => {
                    self.in_scope(
                        || error!(group = %self.meta.group, %reason, "invalid config is ignored"),
                    );
                    let reject = messages::ConfigRejected { reason };
                    let token = extract_response_token::<messages::UpdateConfig>(envelope);
                    self.context.respond(token, Err(reject));
                    return visitor.done();
                }
            },
            messages::SubscribeToActorStatuses { forcing } => {
                let sender = envelope.sender();
                self.in_scope(|| self.subscribe_to_statuses(sender, *forcing));
                return visitor.done();
            }
            messages::Terminate => {
                if self.termination_policy.stop_spawning {
                    let is_newly = !mem::replace(&mut self.control.write().stop_spawning, true);
                    if is_newly {
                        self.in_scope(|| info!("stopped spawning new actors"));
                    }
                }

                self.router.route(&envelope).or(Outcome::Broadcast)
            }
            messages::Ping => {
                self.router.route(&envelope).or(Outcome::Broadcast)
            }
            _ => {
                self.router.route(&envelope).or(Outcome::Discard)
            }
        });

        let guard = EbrGuard::new();
        let start_info = ActorStartInfo::on_message();
        match outcome {
            Outcome::Unicast(key) => match self.get_object_or_spawn(key, start_info, &guard) {
                Some(object) => visitor.visit_last(&object, envelope),
                None => visitor.empty(envelope),
            },
            Outcome::GentleUnicast(key) => match self.get_object(&key, &guard) {
                Some(object) => visitor.visit_last(&object, envelope),
                None => visitor.empty(envelope),
            },
            Outcome::Multicast(list) => {
                let iter = list
                    .into_iter()
                    .filter_map(|key| self.get_object_or_spawn(key, start_info.clone(), &guard));
                self.visit_multiple(envelope, visitor, iter);
            }
            Outcome::GentleMulticast(list) => {
                let iter = list
                    .into_iter()
                    .filter_map(|key| self.get_object(&key, &guard));
                self.visit_multiple(envelope, visitor, iter);
            }
            Outcome::Broadcast => {
                let guard = EbrGuard::new();
                self.visit_multiple(envelope, visitor, self.iter_objects(&guard))
            }
            Outcome::Discard => visitor.empty(envelope),
            Outcome::Default => unreachable!("must be altered earlier"),
        }
    }

    fn get_object(&self, key: &R::Key, guard: &EbrGuard) -> Option<ObjectRef<'_>> {
        self.objects
            .peek(key, guard)
            .and_then(|addr| self.context.book().get(*addr))
    }

    fn get_object_or_spawn<'a>(
        self: &'a Arc<Self>,
        key: R::Key,
        start_info: ActorStartInfo,
        guard: &EbrGuard,
    ) -> Option<ObjectRef<'a>> {
        match self.objects.peek(&key, guard) {
            Some(addr) => Some(*addr),
            None => self.spawn(key, start_info, Default::default(), guard),
        }
        .and_then(|addr| self.context.book().get(addr))
    }

    fn iter_objects<'a: 'g, 'g>(
        &'a self,
        guard: &'g EbrGuard,
    ) -> impl Iterator<Item = ObjectRef<'a>> + 'g {
        self.objects
            .iter(guard)
            .filter_map(move |(_, &addr)| self.context.book().get(addr))
    }

    fn visit_multiple<'a>(
        &'a self,
        envelope: Envelope,
        visitor: &mut dyn GroupVisitor,
        iter: impl Iterator<Item = ObjectRef<'a>>,
    ) {
        let mut iter = iter.peekable();

        if iter.peek().is_none() {
            return visitor.empty(envelope);
        }

        loop {
            let object = iter.next().unwrap();
            if iter.peek().is_none() {
                return visitor.visit_last(&object, envelope);
            } else {
                visitor.visit(&object, &envelope);
            }
        }
    }

    #[inline(never)]
    fn spawn(
        self: &Arc<Self>,
        key: R::Key,
        start_info: ActorStartInfo,
        mut backoff: RestartBackoff,
        guard: &EbrGuard,
    ) -> Option<Addr> {
        let is_restarting = start_info.cause.is_restarted();

        let control = self.control.write();

        if !is_restarting {
            // TODO: use entry API + comment
            if let Some(addr) = self.objects.peek(&key, guard) {
                return Some(*addr);
            }
        }

        if control.stop_spawning {
            return None;
        }

        let group_no = self.context.group().group_no().expect("invalid group addr");
        let entry = self.context.book().vacant_entry(group_no);
        let addr = entry.addr();
        debug_assert_ne!(addr, Addr::NULL);

        let key_2 = key.clone();
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
            .with_key(key.clone())
            .with_config(user_config);

        let sv = self.clone();

        // TODO: move to `harness.rs`.
        let fut = async move {
            let thread = std::thread::current();

            info!(%addr, thread = %thread.name().unwrap_or("?"), "started");

            let object = sv
                .context
                .book()
                .get(addr)
                .expect("where is the current actor?");
            let actor = object.as_actor().expect("a supervisor stores only actors");

            actor.on_start();

            // It must be called after `entry.insert()`.
            let ctx = ctx.with_addr(addr).with_start_info(start_info);
            let fut = AssertUnwindSafe(async { sv.exec.exec(ctx).await.unify() }).catch_unwind();
            let new_status = match fut.await {
                Ok(Ok(())) => ActorStatus::TERMINATED,
                Ok(Err(err)) => ActorStatus::FAILED.with_details(ErrorChain(&*err)),
                Err(panic) => ActorStatus::FAILED.with_details(panic_to_string(panic)),
            };

            let restart_after = {
                // Select the restart policy with the following priority: actor override >
                // config override > blueprint restart policy..
                let default_restart_policy = sv
                    .control
                    .read()
                    .system_config
                    .restart_policy
                    .make_policy()
                    .unwrap_or(sv.restart_policy.clone());
                let restart_policy = actor.restart_policy().unwrap_or(default_restart_policy);

                let restarting_allowed = restart_policy.restarting_allowed(&new_status)
                    && !sv.control.read().stop_spawning;

                actor.set_status(new_status);

                restarting_allowed
                    .then(|| {
                        restart_policy
                            .restart_params()
                            .and_then(|p| backoff.next(&p))
                    })
                    .flatten()
            };

            if let Some(after) = restart_after {
                if after == Duration::ZERO {
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

                sv.spawn(key, ActorStartInfo::on_restart(), backoff, &EbrGuard::new());
            } else {
                debug!("actor won't be restarted");

                // There is a small chance that `spawn()` hasn't been finished yet.
                // So, we need to lock the supervisor to avoid the remove-insert order.
                let _guard = sv.control.read();

                let is_removed = sv.objects.remove(&key);
                debug_assert!(is_removed, "actor is not registered");
            }

            // It's usually the last reference to the actor.
            // Allows the slab to drop the entry immediately below.
            drop(object);

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

        #[cfg(feature = "unstable-stuck-detection")]
        let fut = MeasurePoll::new(fut.instrument(span), self.rt_manager.stuck_detector());
        #[cfg(not(feature = "unstable-stuck-detection"))]
        let fut = MeasurePoll::new(fut.instrument(span));

        rt.spawn(scope.within(fut));

        match self.objects.entry(key_2) {
            HashIndexEntry::Occupied(entry) => {
                debug_assert!(is_restarting);
                entry.update(addr);
            }
            HashIndexEntry::Vacant(entry) => {
                debug_assert!(!is_restarting);
                entry.insert_entry(addr);
            }
        }

        Some(addr)
    }

    fn spawn_on_group_mounted(self: &Arc<Self>, outcome: Outcome<R::Key>) {
        let guard = EbrGuard::new();
        let start_info = ActorStartInfo::on_group_mounted();
        match outcome {
            Outcome::Unicast(key) => {
                self.get_object_or_spawn(key, start_info, &guard);
            }
            Outcome::Multicast(keys) => {
                for key in keys {
                    self.get_object_or_spawn(key, start_info.clone(), &guard);
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

        self.in_scope(|| {
            debug!(
                message = "config updated",
                system = ?control.system_config,
                custom = ?control.user_config.as_ref().unwrap(),
            )
        });
    }

    fn subscribe_to_statuses(&self, sender: Addr, forcing: bool) {
        // Firstly, add the subscriber to handle new objects right way.
        if !self.status_subscription.add(sender) && !forcing {
            // Already subscribed.
            return;
        }

        // Send active statuses to the subscriber.
        let guard = EbrGuard::new();

        // TODO: the same actor can be visited multiple times.
        for (_, &addr) in self.objects.iter(&guard) {
            let Some(object) = self.context.book().get(addr) else {
                // TODO: comment
                continue;
            };

            let actor = object.as_actor().expect("a supervisor stores only actors");
            let result = actor.with_status(|report| self.context.try_send_to(sender, report));

            if result.is_err() {
                // TODO: `unbounded_send(Unsubscribed)`
                warn!(addr = %sender, "status cannot be sent, unsubscribing");
                self.status_subscription.remove(sender);
                break;
            }
        }
    }

    pub(crate) fn finished(self: &Arc<Self>) -> BoxFuture<'static, ()> {
        let sv = self.clone();

        // TODO: added after iter?
        let guard = EbrGuard::new();

        let addrs = self
            .objects
            .iter(&guard)
            .map(|(_, &addr)| addr)
            .collect::<Vec<_>>();

        let fut = async move {
            for addr in addrs {
                let object = ward!(sv.context.book().get(addr), continue);
                let actor = object.as_actor().expect("a supervisor stores only actors");
                actor.finished().await;
            }
        };

        Box::pin(fut)
    }
}

fn extract_response_token<R: Request>(envelope: Envelope) -> ResponseToken<R> {
    msg!(match envelope {
        (R, token) => token,
        _ => unreachable!(),
    })
}

fn panic_to_string(payload: Box<dyn Any>) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        format!("panic: {message}")
    } else if let Some(message) = payload.downcast_ref::<String>() {
        format!("panic: {message}")
    } else {
        "panic: <unsupported payload>".to_string()
    }
}
