use std::{fmt::Display, future::Future, hash::Hash, marker::PhantomData};

use smallbox::smallbox;

use crate::{
    context::Context, exec::ExecResult, object::Object, routers::Router, supervisor::Supervisor,
};

#[derive(Debug)]
pub struct ActorGroup<R, C> {
    router: R,
    _config: PhantomData<C>,
}

impl ActorGroup<(), ()> {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Self {
            router: (),
            _config: PhantomData,
        }
    }
}

impl<R, C> ActorGroup<R, C> {
    pub fn config<C1>(self) -> ActorGroup<R, C1>
    where
        C1: Send + Sync + 'static,
    {
        ActorGroup {
            router: self.router,
            _config: PhantomData,
        }
    }

    pub fn router<R1>(self, router: R1) -> ActorGroup<R1, C>
    where
        R1: Router,
        R1::Key: Clone + Hash + Eq + Send + Sync, // TODO: why is `Sync` required?
    {
        ActorGroup {
            router,
            _config: self._config,
        }
    }

    pub fn exec<X, O, ER>(self, exec: X) -> Schema
    where
        R: Router,
        R::Key: Clone + Hash + Eq + Display + Send + Sync, // TODO: why is `Sync` required?
        X: Fn(Context<C, R::Key>) -> O + Send + Sync + 'static,
        O: Future<Output = ER> + Send + 'static,
        ER: ExecResult,
        // TODO
        C: Send + Sync + 'static,
        /* X: Exec<Context<C, R::Key>>,
         * as Future>::Output: ExecResult, */
    {
        let run = move |ctx: Context, name: String| {
            let ctx = ctx.with_config::<C>();
            let addr = ctx.addr();
            let sv = Supervisor::new(ctx, name, exec, self.router);
            Object::new_group(addr, smallbox!(move |envelope| { sv.route(envelope) }))
        };

        Schema { run: Box::new(run) }
    }
}

pub struct Schema {
    pub(crate) run: Box<dyn FnOnce(Context, String) -> Object>,
}
