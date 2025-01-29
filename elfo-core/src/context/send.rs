#![allow(unreachable_pub)]

use std::{
    future::{Future, IntoFuture},
    marker,
    pin::Pin,
};

use idr_ebr::EbrGuard;
use tracing::trace;

use crate::{
    context::{addrs_with_envelope, DUMPER},
    dumping::{Direction, Dump},
    envelope::MessageKind,
    errors::{SendError, TrySendError},
    Addr, Context, Envelope, Message,
    _priv::Object,
};

use super::e2m;

pub struct Send<'a, M, C, K> {
    ctx: &'a Context<C, K>,
    dest: Option<Addr>,
    msg: M,
}

impl<'a, M, C, K> IntoFuture for Send<'a, M, C, K>
where
    C: marker::Send + marker::Sync,
    K: marker::Sync,
    M: Message,
{
    type IntoFuture = Pin<Box<dyn Future<Output = Self::Output> + marker::Send + 'a>>;
    type Output = Result<(), SendError<M>>;

    fn into_future(self) -> Self::IntoFuture {
        let Self { ctx, dest, msg } = self;
        let kind = MessageKind::regular(ctx.actor_addr);
        if let Some(addr) = dest {
            let fut = async move {
                ctx.do_send_to(addr, msg, kind, |object, envelope| {
                    Object::send(object, addr, envelope)
                })?
                .await
                .map_err(|err| err.map(e2m))
            };
            Box::pin(fut)
        } else {
            Box::pin(ctx.do_send_async(msg, kind))
        }
    }
}

impl<'a, M, C, K> Send<'a, M, C, K>
where
    M: Message,
{
    pub fn unbounded(self) -> Result<(), SendError<M>> {
        let Self { ctx, dest, msg } = self;
        let kind = MessageKind::regular(ctx.actor_addr);

        if let Some(dest) = dest {
            ctx.do_send_to(dest, msg, kind, |obj, env| {
                obj.unbounded_send(dest, env).map_err(|err| err.map(e2m))
            })?
        } else {
            ctx.stats.on_sent_message(&msg);
            trace!("> {msg:?}");
            if let Some(permit) = DUMPER.acquire_m(&msg) {
                permit.record(Dump::message(&msg, &kind, Direction::Out));
            }

            let env = Envelope::new(msg, kind);
            let guard = EbrGuard::new();
            let addrs = ctx.demux.filter(&env);

            let mut success = false;
            let mut unused = None;

            for (addr, env) in addrs_with_envelope(env, &addrs) {
                if let Some(obj) = ctx.book.get(addr, &guard) {
                    match obj.unbounded_send(Addr::NULL, env) {
                        Err(e) => unused = Some(e.into_inner()),
                        Ok(()) => success = true,
                    }
                }
            }

            if success {
                Ok(())
            } else {
                Err(SendError(e2m(unused.unwrap())))
            }
        }
    }

    pub fn try_(self) -> Result<(), TrySendError<M>> {
        if let Some(addr) = self.dest {
            self.ctx.try_send_to(addr, self.msg)
        } else {
            self.ctx.try_send(self.msg)
        }
    }
}

impl<'a, M, C, K> Send<'a, M, C, K> {
    pub fn new(msg: M, ctx: &'a Context<C, K>) -> Self {
        Self {
            msg,
            ctx,
            dest: None,
        }
    }

    pub fn to(mut self, dest: Addr) -> Self {
        self.dest = Some(dest);
        self
    }
}
