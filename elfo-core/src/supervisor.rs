use std::{any::Any, future::Future, panic::AssertUnwindSafe, sync::Arc, time::Duration};

use dashmap::DashMap;
use futures::FutureExt;
use fxhash::FxBuildHasher;
use parking_lot::RwLock;
use tracing::{error, info};

use crate as elfo;
use elfo_macros::{message, msg_raw as msg};
use elfo_utils::{CachePadded, ErrorChain};

use crate::{
    actor::{Actor, ActorStatus},
    addr::Addr,
    config::Config,
    context::Context,
    envelope::Envelope,
    errors::TrySendError,
    exec::{Exec, ExecResult},
    local::Local,
    messages,
    object::{Object, ObjectArc, ObjectMeta},
    routers::{Outcome, Router},
    tls,
    trace_id::TraceId,
};

pub(crate) struct Supervisor<R: Router<C>, C, X> {
    meta: Arc<ObjectMeta>,
    context: Context,
    // TODO: replace with `crossbeam_utils::sync::ShardedLock`?
    objects: DashMap<R::Key, ObjectArc, FxBuildHasher>,
    router: R,
    exec: X,
    control: CachePadded<RwLock<ControlBlock<C>>>,
}

struct ControlBlock<C> {
    config: Option<Arc<C>>,
}

#[message(elfo = crate)]
struct ActorBlocked {
    key: Local<Arc<dyn Any + Send + Sync>>,
}

#[message(elfo = crate)]
struct ActorRestarted {
    key: Local<Arc<dyn Any + Send + Sync>>,
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
            meta: Arc::new(ObjectMeta { group, key: None }),
            context: ctx,
            objects: DashMap::default(),
            router,
            exec,
            control: CachePadded(RwLock::new(control)),
        }
    }

    fn in_scope(&self, f: impl FnOnce()) {
        tls::sync_scope(self.meta.clone(), tls::trace_id(), f);
    }

    pub(crate) fn handle(&self, envelope: Envelope) -> RouteReport {
        msg!(match &envelope {
            ActorBlocked { key } => {
                let key: &R::Key = key.downcast_ref().expect("invalid key");
                let object = ward!(self.objects.get(key), {
                    self.in_scope(|| error!(%key, "blocking removed actor?!"));
                    return RouteReport::Done;
                });
                let actor = object.as_actor().expect("invalid command");
                actor.set_status(ActorStatus::RESTARTING);
                RouteReport::Done
            }
            ActorRestarted { key } => {
                let key: &R::Key = key.downcast_ref().expect("invalid key");
                self.objects.insert(key.clone(), self.spawn(key.clone()));
                RouteReport::Done
            }
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
                    let mut control = self.control.write();
                    let is_first_update = control.config.is_none();
                    control.config = config.get().cloned();
                    self.router
                        .update(&control.config.as_ref().expect("just saved"));
                    self.in_scope(
                        || info!(config = ?control.config.as_ref().unwrap(), "router updated"),
                    );
                    drop(control);
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

    pub(crate) fn do_handle(&self, envelope: Envelope, outcome: Outcome<R::Key>) -> RouteReport {
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

    fn spawn(&self, key: R::Key) -> ObjectArc {
        let entry = self.context.book().vacant_entry();
        let addr = entry.addr();

        let meta = Arc::new(ObjectMeta {
            group: self.meta.group.clone(),
            key: Some(key.to_string()),
        });

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

        let sv_ctx = self.context.pruned();

        // TODO: protect against panics (for `fn(..) -> impl Future`).
        let fut = self.exec.exec(ctx);

        let fut = async move {
            info!(%addr, "started");
            let fut = AssertUnwindSafe(async { fut.await.unify() }).catch_unwind();
            match fut.await {
                Ok(Ok(())) => return info!(%addr, "finished"),
                Ok(Err(err)) => error!(%addr, error = %ErrorChain(&*err), "failed"),
                Err(panic) => error!(%addr, error = %panic_to_string(panic), "panicked"),
            };

            let key = Local::from(Arc::new(key) as Arc<dyn Any + Send + Sync>);
            let message = ActorBlocked { key: key.clone() };
            sv_ctx
                .try_send_to(sv_ctx.addr(), message)
                .expect("cannot block");

            // TODO: use `backoff`.
            tokio::time::sleep(Duration::from_secs(5)).await;

            sv_ctx
                .try_send_to(sv_ctx.addr(), ActorRestarted { key })
                .expect("cannot restart");
        };

        entry.insert(Object::new(addr, Actor::new(addr)));
        let initial_trace_id = TraceId::new(1).unwrap(); // TODO: set initial trace ids.
        tokio::spawn(tls::scope(meta, initial_trace_id, fut));
        self.context.book().get_owned(addr).expect("just created")
    }

    fn spawn_by_outcome(&self, outcome: Outcome<R::Key>) {
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
        (*message).to_string()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "(unsupported payload)".to_string()
    }
}

pub(crate) enum RouteReport {
    Done,
    Closed(Envelope),
    Wait(Addr, Envelope),
    WaitAll(bool, Vec<(Addr, Envelope)>),
}
