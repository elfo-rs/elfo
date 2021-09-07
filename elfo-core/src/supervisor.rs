use std::{any::Any, future::Future, panic::AssertUnwindSafe, sync::Arc, time::Duration};

use dashmap::DashMap;
use futures::FutureExt;
use fxhash::FxBuildHasher;
use parking_lot::RwLock;
use tracing::{error_span, info, Instrument, Span};

use crate as elfo;
use elfo_macros::msg_raw as msg;
use elfo_utils::{CachePadded, ErrorChain};

use crate::{
    actor::{Actor, ActorStatus},
    addr::Addr,
    config::Config,
    context::Context,
    envelope::Envelope,
    errors::TrySendError,
    exec::{Exec, ExecResult},
    messages,
    object::{Object, ObjectArc, ObjectMeta},
    permissions::AtomicPermissions,
    routers::{Outcome, Router},
    scope::{self, Scope},
};

pub(crate) struct Supervisor<R: Router<C>, C, X> {
    meta: Arc<ObjectMeta>,
    span: Span,
    context: Context,
    // TODO: replace with `crossbeam_utils::sync::ShardedLock`?
    objects: DashMap<R::Key, ObjectArc, FxBuildHasher>,
    router: R,
    exec: X,
    control: CachePadded<RwLock<ControlBlock<C>>>,
    permissions: Arc<AtomicPermissions>,
}

struct ControlBlock<C> {
    config: Option<Arc<C>>,
}

macro_rules! get_or_spawn {
    ($this:ident, $key:expr) => {{
        let key = $key;
        ward!($this.objects.get(&key), {
            $this
                .objects
                .entry(key.clone())
                .or_insert_with(|| $this.spawn(key))
                .downgrade()
        })
    }};
}

impl<R, C, X> Supervisor<R, C, X>
where
    R: Router<C>,
    X: Exec<Context<C, R::Key>>,
    <X::Output as Future>::Output: ExecResult,
    C: Config,
{
    pub(crate) fn new(ctx: Context, group: String, exec: X, router: R) -> Self {
        let control = ControlBlock { config: None };

        Self {
            span: error_span!(parent: Span::none(), "", actor_group = group.as_str()),
            meta: Arc::new(ObjectMeta { group, key: None }),
            context: ctx,
            objects: DashMap::default(),
            router,
            exec,
            control: CachePadded(RwLock::new(control)),
            permissions: Default::default(),
        }
    }

    fn in_scope(&self, f: impl FnOnce()) {
        Scope::with_trace_id(
            scope::trace_id(),
            self.context.addr(),
            self.context.group(), // `Addr::NULL`, actually
            self.meta.clone(),
            self.permissions.clone(),
        )
        .sync_within(|| self.span.in_scope(f));
    }

    pub(crate) fn handle(self: &Arc<Self>, envelope: Envelope) -> RouteReport {
        msg!(match &envelope {
            messages::ValidateConfig { config } => match config.decode::<C>() {
                Ok(config) => {
                    let is_first_update = self.control.write().config.is_none();
                    if !is_first_update {
                        let outcome = self.router.route(&envelope);
                        let mut envelope = envelope;
                        envelope.set_message(messages::ValidateConfig { config });
                        self.do_handle(envelope, outcome.or(Outcome::Broadcast))
                    } else {
                        RouteReport::Done
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

                    let system = config.get_system();

                    // Update the dumper's config.
                    self.context.dumper().configure(&system.dumping);

                    // Update telemetry's config.
                    let mut perm = self.permissions.load();
                    perm.set_telemetry_per_actor_group_enabled(system.telemetry.per_actor_group);
                    perm.set_telemetry_per_actor_key_enabled(system.telemetry.per_actor_key);
                    self.permissions.store(perm);

                    // Update user's config.
                    let is_first_update = control.config.is_none();
                    control.config = Some(config.get_user::<C>().clone());
                    self.router
                        .update(control.config.as_ref().expect("just saved"));
                    self.in_scope(
                        || info!(config = ?control.config.as_ref().unwrap(), "router updated"),
                    );

                    // Release the lock.
                    drop(control);

                    // Send `UpdateConfig` across actors.
                    let outcome = self.router.route(&envelope);
                    if !is_first_update {
                        let mut envelope = envelope;
                        envelope.set_message(messages::UpdateConfig { config });
                        self.do_handle(envelope, outcome.or(Outcome::Broadcast))
                    } else {
                        self.spawn_by_outcome(outcome);
                        RouteReport::Done
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
            _ => {
                let outcome = self.router.route(&envelope);
                self.do_handle(envelope, outcome)
            }
        })
    }

    pub(crate) fn do_handle(
        self: &Arc<Self>,
        envelope: Envelope,
        outcome: Outcome<R::Key>,
    ) -> RouteReport {
        // TODO: avoid copy & paste.
        match outcome {
            Outcome::Unicast(key) => {
                let object = get_or_spawn!(self, key);
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
                    let object = get_or_spawn!(self, key);

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

    fn spawn(self: &Arc<Self>, key: R::Key) -> ObjectArc {
        let entry = self.context.book().vacant_entry();
        let addr = entry.addr();

        let key_str = key.to_string();
        let span = error_span!(
            parent: Span::none(),
            "",
            actor_group = self.meta.group.as_str(),
            actor_key = key_str.as_str()
        );

        let control = self.control.read();
        let config = control.config.as_ref().cloned().expect("config is unset");

        let ctx = self
            .context
            .clone()
            .with_addr(addr)
            .with_group(self.context.addr())
            .with_key(key.clone())
            .with_config(config);

        drop(control);

        let sv = self.clone();

        // TODO: protect against panics (for `fn(..) -> impl Future`).
        let fut = self.exec.exec(ctx);

        // TODO: move to `harness.rs`.
        let fut = async move {
            info!(%addr, "started");

            let fut = AssertUnwindSafe(async { fut.await.unify() }).catch_unwind();
            let new_status = match fut.await {
                Ok(Ok(())) => ActorStatus::TERMINATED,
                Ok(Err(err)) => ActorStatus::FAILED.with_details(ErrorChain(&*err)),
                Err(panic) => ActorStatus::FAILED.with_details(panic_to_string(panic)),
            };

            let need_to_restart = new_status.is_failed();

            sv.objects
                .get(&key)
                .expect("where is the current actor?")
                .as_actor()
                .expect("a supervisor stores only actors")
                .set_status(new_status);

            if need_to_restart {
                // TODO: use `backoff`.
                tokio::time::sleep(Duration::from_secs(5)).await;
                sv.objects.insert(key.clone(), sv.spawn(key))
            } else {
                sv.objects.remove(&key).map(|(_, v)| v)
            }
            .expect("where is the current actor?");

            sv.context.book().remove(addr);
        };

        entry.insert(Object::new(addr, Actor::new(addr)));

        let meta = Arc::new(ObjectMeta {
            group: self.meta.group.clone(),
            key: Some(key_str),
        });

        let scope = Scope::new(addr, self.context.addr(), meta, self.permissions.clone());
        tokio::spawn(scope.within(fut.instrument(span)));
        self.context.book().get_owned(addr).expect("just created")
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
