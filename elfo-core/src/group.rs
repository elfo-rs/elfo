use std::{fmt::Debug, future::Future, marker::PhantomData};

use smallbox::smallbox;

use crate::{
    config::Config,
    context::Context,
    exec::ExecResult,
    object::{Group, Object},
    routers::Router,
    supervisor::Supervisor,
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
    pub fn config<C1: Config>(self) -> ActorGroup<R, C1> {
        ActorGroup {
            router: self.router,
            _config: PhantomData,
        }
    }

    pub fn router<R1: Router<C>>(self, router: R1) -> ActorGroup<R1, C> {
        ActorGroup {
            router,
            _config: self._config,
        }
    }

    pub fn exec<X, O, ER>(self, exec: X) -> Schema
    where
        R: Router<C>,
        X: Fn(Context<C, R::Key>) -> O + Send + Sync + 'static,
        O: Future<Output = ER> + Send + 'static,
        ER: ExecResult,
        C: Config,
    {
        let run = move |ctx: Context, name: String| {
            let addr = ctx.addr();
            let sv = Supervisor::new(ctx, name, exec, self.router);
            let router = smallbox!(move |envelope| { sv.handle(envelope) });
            Object::new(addr, Group::new(router))
        };

        Schema { run: Box::new(run) }
    }
}

pub struct Schema {
    pub(crate) run: Box<dyn FnOnce(Context, String) -> Object>,
}
