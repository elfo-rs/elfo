use std::{
    any::Any,
    future::Future,
    pin::Pin,
    task::{self, Poll, Waker},
};

use pin_project::pin_project;
use tokio::time::{Duration, Instant, Sleep};

use crate::{
    addr::Addr,
    envelope::{Envelope, MessageKind},
    message::Message,
    source::{SourceArc, SourceStream, Unattached},
    tracing::TraceId,
};

/// A source that emits messages periodically.
pub struct Interval<M> {
    source: SourceArc<IntervalSource<M>>,
}

const NEVER: Duration = Duration::new(0, 0);

#[pin_project]
struct IntervalSource<M> {
    waker: Option<Waker>,
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
            waker: None,
            message,
            period: NEVER,
            is_delayed: false,
            sleep: tokio::time::sleep_until(Instant::now()),
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

    /// Configures the period of ticks.
    /// It differs from [`Interval::start`] because TODO
    ///
    /// has no effect if not started
    ///
    /// # Panics
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

            if let Some(waker) = source.waker.take() {
                waker.wake();
            }
        }

        *source.period = period;
    }

    /// Schedules the interval to start emitting ticks after `period`.
    ///
    /// # Panics
    /// If `period` is zero.
    #[track_caller]
    pub fn start(&self, period: Duration) {
        assert_ne!(period, NEVER, "period must be non-zero");
        self.schedule(None, period);
    }

    /// Schedules the interval to start emitting ticks after `delay`.
    ///
    /// # Panics
    /// If `period` is zero.
    #[track_caller]
    pub fn start_after(&self, delay: Duration, period: Duration) {
        assert_ne!(period, NEVER, "period must be non-zero");
        self.schedule(Some(Instant::now() + delay), period);
    }

    /// Schedules the interval to start emitting ticks at `when`.
    ///
    /// # Panics
    /// If `period` is zero.
    ///
    /// # Stability
    /// This method is unstable, because it accepts [`tokio::time::Instant`],
    /// which will be replaced in the future to support other runtimes.
    #[stability::unstable]
    #[track_caller]
    pub fn start_at(&self, when: Instant, period: Duration) {
        assert_ne!(period, NEVER, "period must be non-zero");
        self.schedule(Some(when), period);
    }

    /// Stops any ticks.
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

        if let Some(waker) = source.waker.take() {
            waker.wake();
        }
    }
}

impl<M: Message> SourceStream for IntervalSource<M> {
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn poll_recv(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<Option<Envelope>> {
        let mut this = self.project();

        *this.waker = Some(cx.waker().clone());

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
        let new_deadline = this.sleep.deadline() + *this.period;
        this.sleep.reset(new_deadline);

        // Emit a message.
        let message = this.message.clone();
        let kind = MessageKind::Regular { sender: Addr::NULL };
        let trace_id = TraceId::generate();
        let envelope = Envelope::with_trace_id(message, kind, trace_id).upcast();

        Poll::Ready(Some(envelope))
    }
}

fn far_future() -> Instant {
    // Copied from `tokio`.
    // Roughly 30 years from now.
    // API does not provide a way to obtain max `Instant`
    // or convert specific date in the future to instant.
    // 1000 years overflows on macOS, 100 years overflows on FreeBSD.
    Instant::now() + Duration::from_secs(86400 * 365 * 30)
}
