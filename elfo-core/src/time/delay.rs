use std::{
    any::Any,
    future::Future,
    pin::Pin,
    task::{self, Poll},
};

use pin_project::pin_project;
use sealed::sealed;
use tokio::time::{Duration, Instant, Sleep};

use crate::{
    addr::Addr,
    envelope::{Envelope, MessageKind},
    message::Message,
    scope,
    source::{SourceArc, SourceStream, Unattached},
    time::far_future,
    tracing::TraceId,
};

/// A source that emits a message after some specified time.
///
/// # Tracing
///
/// The emitted message continues the current trace.
///
/// # Example
///
/// ```
/// # use std::time::Duration;
/// # use elfo_core as elfo;
/// # struct Config { delay: Duration }
/// # async fn exec(mut ctx: elfo::Context<Config>) {
/// # use elfo::{message, msg};
/// # #[message]
/// # struct SomeEvent;
/// use elfo::time::Delay;
///
/// #[message]
/// struct MyTick;
///
/// while let Some(envelope) = ctx.recv().await {
///     msg!(match envelope {
///         SomeEvent => {
///             ctx.attach(Delay::new(ctx.config().delay, MyTick));
///         },
///         MyTick => {
///             tracing::info!("tick!");
///         },
///     });
/// }
/// # }
/// ```
pub struct Delay<M> {
    source: SourceArc<DelaySource<M>>,
}

#[sealed]
impl<M: Message> crate::source::SourceHandle for Delay<M> {
    fn is_terminated(&self) -> bool {
        self.source.is_terminated()
    }

    fn terminate(self) {
        // Reset the sleep timer to prevent it from waking up.
        let mut guard = self.source.lock();
        guard.pinned().project().sleep.reset(far_future());
        drop(guard);

        self.source.terminate();
    }
}

#[pin_project]
struct DelaySource<M> {
    message: Option<M>,
    trace_id: Option<TraceId>,
    #[pin]
    sleep: Sleep,
}

impl<M: Message> Delay<M> {
    /// Schedules the timer to emit the provided message after `delay`.
    ///
    /// Creates an unattached instance of [`Delay`].
    pub fn new(delay: Duration, message: M) -> Unattached<Self> {
        Self::until(Instant::now() + delay, message)
    }

    /// Schedules the timer to emit the provided message at `when`.
    ///
    /// Creates an unattached instance of [`Delay`].
    ///
    /// # Stability
    ///
    /// This method is unstable, because it accepts [`tokio::time::Instant`],
    /// which will be replaced in the future to support other runtimes.
    #[stability::unstable]
    pub fn until(when: Instant, message: M) -> Unattached<Self> {
        let source = SourceArc::new(DelaySource {
            message: Some(message),
            trace_id: Some(scope::trace_id()),
            sleep: tokio::time::sleep_until(when),
        });

        Unattached::new(source.clone(), Self { source })
    }
}

impl<M: Message> SourceStream for DelaySource<M> {
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn poll_recv(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<Option<Envelope>> {
        let mut this = self.project();

        // Terminate if the message is already emitted.
        if this.message.is_none() {
            return Poll::Ready(None);
        }

        // Wait for a tick from implementation.
        if !this.sleep.as_mut().poll(cx).is_ready() {
            return Poll::Pending;
        }

        // Emit the message.
        let message = this.message.take().unwrap();
        let kind = MessageKind::Regular { sender: Addr::NULL };
        let trace_id = this.trace_id.take().unwrap_or_else(TraceId::generate);
        let envelope = Envelope::with_trace_id(message, kind, trace_id).upcast();

        Poll::Ready(Some(envelope))
    }
}
