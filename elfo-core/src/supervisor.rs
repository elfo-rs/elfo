use std::{any::Any, fmt::Display, future::Future, hash::Hash, panic::AssertUnwindSafe};

use dashmap::DashMap;
use futures::FutureExt;
use fxhash::FxBuildHasher;
use tracing::{error, error_span, Instrument, Span};

use crate::{
    addr::Addr,
    context::Context,
    envelope::Envelope,
    exec::{BoxedError, Exec, ExecResult},
    mailbox::TrySendError,
    object::{Object, ObjectArc},
    routers::{Outcome, Router},
};

pub(crate) struct Supervisor<R: Router, C, X> {
    name: String,
    context: Context<C, ()>,
    objects: DashMap<R::Key, ObjectArc, FxBuildHasher>,
    router: R,
    exec: X,
}

impl<R, C, X> Supervisor<R, C, X>
where
    R: Router,
    R::Key: Clone + Hash + Eq + Display,
    X: Exec<Context<C, R::Key>>,
    <X::Output as Future>::Output: ExecResult,
{
    pub(crate) fn new(ctx: Context<C, ()>, name: String, exec: X, router: R) -> Self {
        Self {
            name,
            context: ctx,
            objects: DashMap::default(),
            router,
            exec,
        }
    }

    pub(crate) fn route(&self, envelope: Envelope) -> RouteReport {
        let outcome = self.router.route(&envelope);

        match outcome {
            Outcome::Unicast(key) => {
                let object = ward!(self.objects.get(&key), {
                    self.objects
                        .entry(key.clone())
                        .or_insert_with(|| self.spawn(key))
                        .downgrade()
                });

                let mailbox = object.mailbox().expect("supervisor stores only actors");
                match mailbox.try_send(envelope) {
                    Ok(()) => RouteReport::Done,
                    Err(TrySendError::Full(envelope)) => RouteReport::Wait(object.addr(), envelope),
                    Err(TrySendError::Closed(envelope)) => RouteReport::Closed(envelope),
                }
            }
            Outcome::Broadcast => {
                let mut waiters = Vec::new();

                for object in self.objects.iter() {
                    let mailbox = object.mailbox().expect("supervisor stores only actors");

                    // TODO: we shouldn't clone `envelope` for the last object in a sequence.
                    if let Err(TrySendError::Full(envelope)) = mailbox.try_send(envelope.clone()) {
                        waiters.push((object.addr(), envelope));
                    }
                }

                if waiters.is_empty() {
                    RouteReport::Closed(envelope)
                } else {
                    RouteReport::WaitAll(waiters)
                }
            }
            Outcome::Discard => RouteReport::Done, // TODO: should it be the `Closed` variant?
        }
    }

    fn on_actor_finished(&self, addr: Addr, result: ActorResult) {
        match result {
            ActorResult::Completed => {}
            ActorResult::Failed(error) => {
                error!(%error, "actor failed");
            }
            ActorResult::Panicked(panic) => {
                let error = payload_to_string(&*panic);
                error!(%error, "actor panicked");
            }
        }
    }

    fn spawn(&self, key: R::Key) -> ObjectArc {
        let addr = self.context.book().insert_with_addr(move |addr| {
            let full_name = format!("{}.{}", self.name, key);
            let span = error_span!(parent: Span::none(), "", actor = %full_name);
            let _entered = span.enter();

            // TODO: protect against panics (for `fn(..) -> impl Future`).
            let ctx = self.context.clone();
            let fut = self.exec.exec(ctx.child(addr, key.clone()));
            let fut = async move {
                let fut = AssertUnwindSafe(async { fut.await.unify() }).catch_unwind();
                let _res = match fut.await {
                    Ok(Ok(())) => ActorResult::Completed,
                    Ok(Err(err)) => ActorResult::Failed(err),
                    Err(panic) => ActorResult::Panicked(panic),
                };

                // TODO
            };

            drop(_entered);
            let handle = tokio::spawn(fut.instrument(span));

            Object::new_actor(addr, handle)
        });

        self.context.book().get_owned(addr).expect("just created")
    }
}

enum ActorResult {
    Completed,
    Failed(BoxedError),
    Panicked(Box<dyn Any + Send + 'static>),
}

fn payload_to_string(payload: &dyn Any) -> String {
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
    WaitAll(Vec<(Addr, Envelope)>),
}
