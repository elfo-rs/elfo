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
    source::{SourceArc, SourceStream, Unattached},
    time::far_future,
    tracing::TraceId,
};

/// A source that emits messages periodically.
/// Clones the message on every tick.
///
/// # Tracing
///
/// Every message starts a new trace, thus a new trace id is generated and
/// assigned to the current scope.
///
/// # Example
///
/// ```
/// # use std::time::Duration;
/// # use elfo_core as elfo;
/// # struct Config { period: Duration }
/// # async fn exec(mut ctx: elfo::Context<Config>) {
/// # use elfo::{message, msg};
/// use elfo::{time::Interval, messages::ConfigUpdated};
///
/// #[message]
/// struct MyTick;
///
/// let interval = ctx.attach(Interval::new(MyTick));
/// interval.start(ctx.config().period);
///
/// while let Some(envelope) = ctx.recv().await {
///     msg!(match envelope {
///         ConfigUpdated => {
///             interval.set_period(ctx.config().period);
///         },
///         MyTick => {
///             tracing::info!("tick!");
///         },
///     });
/// }
/// # }
/// ```
pub struct Interval<M> {
    source: SourceArc<IntervalSource<M>>,
}

#[sealed]
impl<M: Message> crate::source::SourceHandle for Interval<M> {
    fn is_terminated(&self) -> bool {
        self.source.is_terminated()
    }

    fn terminate(self) {
        // Reset the sleep timer to prevent it from waking up.
        self.stop();
        self.source.terminate();
    }
}

const NEVER: Duration = Duration::new(0, 0);

#[pin_project]
struct IntervalSource<M> {
    message: M,
    period: Duration,
    is_delayed: bool,
    #[pin]
    sleep: Sleep,
}

impl<M: Message> Interval<M> {
    /// Creates an unattached instance of [`Interval`].
    pub fn new(message: M) -> Unattached<Self> {
        let source = SourceArc::new(IntervalSource {
            message,
            period: NEVER,
            is_delayed: false,
            sleep: tokio::time::sleep_until(far_future()),
        });

        Unattached::new(source.clone(), Self { source })
    }

    /// Replaces a stored message with the provided one.
    pub fn set_message(&self, message: M) {
        let mut guard = self.source.lock();
        let source = guard.pinned().project();
        *source.message = message;
    }

    // TODO: pub fn set_missed_tick_policy

    /// Configures the period of ticks. Intended to be called on
    /// `ConfigUpdated`.
    ///
    /// Does nothing if the timer is not started or the period hasn't been
    /// changed.
    ///
    /// Unlike rescheduling (`start_*` methods), it only adjusts the current
    /// period and doesn't change the time origin. For instance, if we have
    /// a configured interval with period = 5s and try to call one of these
    /// methods, the difference looks something like this:
    ///
    /// ```text
    /// set_period(10s): | 5s | 5s | 5s |  # 10s  |   10s   |
    /// start(10s):      | 5s | 5s | 5s |  #   10s   |   10s   |
    ///                                    #
    ///                               called here
    /// ```
    ///
    /// # Panics
    ///
    /// If `period` is zero.
    #[track_caller]
    pub fn set_period(&self, period: Duration) {
        assert_ne!(period, NEVER, "period must be non-zero");

        let mut guard = self.source.lock();
        let source = guard.pinned().project();

        // Do nothing if not started or the period hasn't been changed.
        if *source.period == NEVER || period == *source.period {
            return;
        }

        // Reschedule if inside the period.
        if !*source.is_delayed {
            let new_deadline = source.sleep.deadline() - *source.period + period;
            source.sleep.reset(new_deadline);
            *source.period = period;
            guard.wake();
        } else {
            *source.period = period;
        }
    }

    /// Schedules the timer to start emitting ticks every `period`.
    /// The first tick will be emitted also after `period`.
    ///
    /// Reschedules the timer if it's already started.
    ///
    /// # Panics
    ///
    /// If `period` is zero.
    #[track_caller]
    pub fn start(&self, period: Duration) {
        assert_ne!(period, NEVER, "period must be non-zero");
        self.schedule(None, period);
    }

    /// Schedules the timer to start emitting ticks every `period`.
    /// The first tick will be emitted after `delay`.
    ///
    /// Reschedules the timer if it's already started.
    ///
    /// # Panics
    ///
    /// If `period` is zero.
    #[track_caller]
    pub fn start_after(&self, delay: Duration, period: Duration) {
        self.start_at(Instant::now() + delay, period);
    }

    /// Schedules the timer to start emitting ticks every `period`.
    /// The first tick will be emitted at `when`.
    ///
    /// Reschedules the timer if it's already started.
    ///
    /// # Panics
    ///
    /// If `period` is zero.
    ///
    /// # Stability
    ///
    /// This method is unstable, because it accepts [`tokio::time::Instant`],
    /// which will be replaced in the future to support other runtimes.
    #[stability::unstable]
    #[track_caller]
    pub fn start_at(&self, when: Instant, period: Duration) {
        assert_ne!(period, NEVER, "period must be non-zero");
        self.schedule(Some(when), period);
    }

    /// Stops any ticks. To resume ticks use one of `start_*` methods.
    ///
    /// Note: it doesn't terminates the source. It means the source is present
    /// in the source map until [`SourceHandle::terminate()`] is called.
    ///
    /// [`SourceHandle::terminate()`]: crate::SourceHandle::terminate()
    pub fn stop(&self) {
        self.schedule(Some(far_future()), NEVER);
    }

    fn schedule(&self, when: Option<Instant>, period: Duration) {
        let mut guard = self.source.lock();
        let source = guard.pinned().project();

        *source.is_delayed = when.is_some();
        *source.period = period;

        let new_deadline = when.unwrap_or_else(|| Instant::now() + period);
        source.sleep.reset(new_deadline);
        guard.wake();
    }
}

impl<M: Message> SourceStream for IntervalSource<M> {
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn poll_recv(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<Option<Envelope>> {
        let mut this = self.project();

        // Do nothing if stopped or not configured.
        if *this.period == NEVER {
            return Poll::Pending;
        }

        // Wait for a tick from implementation.
        if !this.sleep.as_mut().poll(cx).is_ready() {
            return Poll::Pending;
        }

        // After first tick, the interval isn't delayed even if it was.
        *this.is_delayed = false;

        // Reset the underlying timer.
        // It would be nice to use `reset_without_reregister` here, but it's private.
        // TODO: consider moving to `tokio::time::Interval`, which uses it internally.
        let new_deadline = this.sleep.deadline() + *this.period;
        this.sleep.reset(new_deadline);

        // Emit the message.
        let message = this.message.clone();
        let kind = MessageKind::Regular { sender: Addr::NULL };
        let trace_id = TraceId::generate();
        let envelope = Envelope::with_trace_id(message, kind, trace_id).upcast();

        Poll::Ready(Some(envelope))
    }
}
