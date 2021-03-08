use std::{fmt::Display, future::Future, hash::Hash, marker::PhantomData};

use smallbox::{smallbox, SmallBox};

use crate::{
    addr::Addr,
    context::Context,
    envelope::Envelope,
    exec::{Exec, ExecResult},
    object::Object,
    routers::Router,
    supervisor::{RouteReport, Supervisor},
};

#[derive(Debug)]
pub struct ActorGroup<R, C, X> {
    name: String,
    router: R,
    exec: X,
    _config: PhantomData<C>,
}

impl ActorGroup<(), (), ()> {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Self {
            name: "<unnamed>".into(),
            router: (),
            exec: (),
            _config: PhantomData,
        }
    }
}

impl<R, C, X> ActorGroup<R, C, X> {
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    pub fn config<C1>(self) -> ActorGroup<R, C1, X>
    where
        C1: Send + Sync + 'static,
    {
        ActorGroup {
            name: self.name,
            router: self.router,
            exec: self.exec,
            _config: PhantomData,
        }
    }

    pub fn router<R1>(self, router: R1) -> ActorGroup<R1, C, X>
    where
        R1: Router,
        R1::Key: Clone + Hash + Eq + Send + Sync, // TODO: why is `Sync` required?
    {
        ActorGroup {
            name: self.name,
            router,
            exec: self.exec,
            _config: self._config,
        }
    }

    pub fn exec<X1>(self, exec: X1) -> ActorGroup<R, C, X1>
    where
        R: Router,
        X1: Exec<Context<C, R::Key>>,
        <X1::Output as Future>::Output: ExecResult,
    {
        ActorGroup {
            name: self.name,
            router: self.router,
            exec,
            _config: self._config,
        }
    }

    pub fn spawn<PC, PK>(self, ctx: &Context<PC, PK>) -> Addr
    where
        R: Router,
        R::Key: Clone + Hash + Eq + Display + Send + Sync, // TODO: why is `Sync` required?
        C: Send + Sync + 'static,
        X: Exec<Context<C, R::Key>>,
        <X::Output as Future>::Output: ExecResult,
    {
        ctx.book().insert_with_addr(|addr| {
            let ctx: Context<C, _> = ctx.child(addr, ());
            let sv = Supervisor::new(ctx, self.name, self.exec, self.router);
            Object::new_group(addr, smallbox!(move |envelope| { sv.route(envelope) }))
        })
    }
}

pub(crate) type GroupRouter = SmallBox<dyn Fn(Envelope) -> RouteReport + Send + Sync, [u8; 160]>;
