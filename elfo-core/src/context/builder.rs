use std::future::{Future, IntoFuture};

use idr_ebr::EbrGuard;
use tracing::trace;

use crate::{
    context::{addrs_with_envelope, e2m, DUMPER},
    errors::{SendError, TrySendError},
    Addr, Context, Envelope, Message,
    _priv::MessageKind,
    dumping::{Direction, Dump},
};

/// Builder for specifying how message should be sent.
pub struct SendMsg<'cx, M, C, K, F> {
    opts: Opts<M>,
    ctx: &'cx Context<C, K>,
    send: F,
}

impl<'cx, F, M, C, K> IntoFuture for SendMsg<'cx, M, C, K, F>
where
    M: Message,
    C: 'static,
    K: 'static,
    F: SendFn<'cx, M, C, K>,
{
    type IntoFuture = F::Fut;
    type Output = <F::Fut as Future>::Output;

    fn into_future(self) -> Self::IntoFuture {
        let Self { send, opts, ctx } = self;
        send(opts, ctx)
    }
}

impl<M, C, K, F> SendMsg<'_, M, C, K, F>
where
    M: Message,
{
    pub fn try_(self) -> Result<(), TrySendError<M>> {
        // XXX: avoid duplication with `unbounded_send()` and `send()`.
        let Self { opts, ctx, send: _ } = self;
        let Opts { dst, msg } = opts;

        let kind = MessageKind::regular(ctx.actor_addr);
        if let Some(dst) = dst {
            return ctx.do_send_to(dst, msg, kind, |obj, env| {
                obj.try_send(dst, env).map_err(|err| err.map(e2m))
            })?;
        }

        ctx.stats.on_sent_message(&msg); // TODO: only if successful?

        trace!("> {:?}", msg);
        if let Some(permit) = DUMPER.acquire_m(&msg) {
            permit.record(Dump::message(&msg, &kind, Direction::Out));
        }

        let envelope = Envelope::new(msg, kind);
        let addrs = ctx.demux.filter(&envelope);

        if addrs.is_empty() {
            return Err(TrySendError::Closed(e2m(envelope)));
        }

        let guard = EbrGuard::new();

        if addrs.len() == 1 {
            return match ctx.book.get(addrs[0], &guard) {
                Some(object) => object
                    .try_send(Addr::NULL, envelope)
                    .map_err(|err| err.map(e2m)),
                None => Err(TrySendError::Closed(e2m(envelope))),
            };
        }

        let mut unused = None;
        let mut has_full = false;
        let mut success = false;

        for (addr, envelope) in addrs_with_envelope(envelope, &addrs) {
            match ctx.book.get(addr, &guard) {
                Some(object) => match object.try_send(Addr::NULL, envelope) {
                    Ok(()) => success = true,
                    Err(err) => {
                        has_full |= err.is_full();
                        unused = Some(err.into_inner());
                    }
                },
                None => unused = Some(envelope),
            };
        }

        if success {
            Ok(())
        } else if has_full {
            Err(TrySendError::Full(e2m(unused.unwrap())))
        } else {
            Err(TrySendError::Closed(e2m(unused.unwrap())))
        }
    }

    pub fn unbounded(self) -> Result<(), SendError<M>> {
        let Self { ctx, opts, send: _ } = self;
        let Opts { dst, msg } = opts;

        let kind = MessageKind::regular(ctx.actor_addr);
        if let Some(dst) = dst {
            return ctx.do_send_to(dst, msg, kind, |obj, env| {
                obj.unbounded_send(dst, env).map_err(|err| err.map(e2m))
            })?;
        }

        ctx.stats.on_sent_message(&msg); // TODO: only if successful?

        trace!("> {:?}", msg);
        if let Some(permit) = DUMPER.acquire_m(&msg) {
            permit.record(Dump::message(&msg, &kind, Direction::Out));
        }

        let envelope = Envelope::new(msg, kind);
        let addrs = ctx.demux.filter(&envelope);

        if addrs.is_empty() {
            return Err(SendError(e2m(envelope)));
        }

        let guard = EbrGuard::new();

        if addrs.len() == 1 {
            return match ctx.book.get(addrs[0], &guard) {
                Some(object) => object
                    .unbounded_send(Addr::NULL, envelope)
                    .map_err(|err| err.map(e2m)),
                None => Err(SendError(e2m(envelope))),
            };
        }

        let mut unused = None;
        let mut success = false;

        for (addr, envelope) in addrs_with_envelope(envelope, &addrs) {
            match ctx.book.get(addr, &guard) {
                Some(object) => match object.unbounded_send(Addr::NULL, envelope) {
                    Ok(()) => success = true,
                    Err(err) => unused = Some(err.into_inner()),
                },
                None => unused = Some(envelope),
            };
        }

        if success {
            Ok(())
        } else {
            Err(SendError(e2m(unused.unwrap())))
        }
    }
}

impl<'cx, M, C, K, F> SendMsg<'cx, M, C, K, F> {
    pub(crate) fn new(send: F, ctx: &'cx Context<C, K>, msg: M) -> Self {
        Self {
            ctx,
            send,
            opts: Opts { msg, dst: None },
        }
    }

    /// Specify destination address.
    pub fn to(mut self, addr: Addr) -> Self {
        self.opts.dst = Some(addr);
        self
    }
}

/// Trait alias used to work-around lack of RTN or `impl_trait_in_assoc_type`.
///
/// Internal, don't rely on it.
#[doc(hidden)]
pub trait SendFn<'cx, M, C, K>
where
    C: 'static,
    K: 'static,
    Self: FnOnce(Opts<M>, &'cx Context<C, K>) -> Self::Fut,
{
    /// Future which function has to return.
    type Fut: Future<Output = Result<(), SendError<M>>> + Send;
}

impl<'cx, M, C, K, Fut, F> SendFn<'cx, M, C, K> for F
where
    C: 'static,
    K: 'static,
    F: FnOnce(Opts<M>, &'cx Context<C, K>) -> Fut,
    Fut: Future<Output = Result<(), SendError<M>>> + Send,
{
    type Fut = Fut;
}

/// Options. Internal API, don't use it.
#[doc(hidden)]
pub struct Opts<M> {
    pub(crate) msg: M,
    pub(crate) dst: Option<Addr>,
}
