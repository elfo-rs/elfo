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

// TODO: revise name and merge with `Stopwatch`.
pub struct Interval<M> {
    source: SourceArc<IntervalSource<M>>,
}

#[pin_project]
struct IntervalSource<M> {
    waker: Option<Waker>,
    message: M,
    #[pin]
    sleep: Sleep,
    start_at: Option<Instant>,
    period: Duration,
}

impl<M: Message> Interval<M> {
    pub fn new(message: M) -> Unattached<Self> {
        let source = SourceArc::new(IntervalSource {
            waker: None,
            message,
            sleep: tokio::time::sleep_until(Instant::now()),
            start_at: None,
            period: Duration::new(0, 0),
        });

        Unattached::new(source.clone(), Self { source })
    }

    // TODO: set_message

    // TODO: &self
    pub fn after(self, after: Duration) -> Self {
        let when = Instant::now() + after;
        let mut guard = self.source.lock();
        let source = guard.pinned().project();
        *source.start_at = Some(when);
        source.sleep.reset(when);

        if let Some(waker) = source.waker.take() {
            waker.wake();
        }

        drop(guard);
        self
    }

    pub fn set_period(&self, new_period: Duration) {
        assert!(new_period > Duration::new(0, 0));

        let mut guard = self.source.lock();
        let source = guard.pinned().project();

        if new_period == *source.period {
            return;
        }

        *source.period = new_period;

        if source.start_at.is_none() {
            let new_deadline = source.sleep.deadline() - *source.period + new_period;
            source.sleep.reset(new_deadline);

            if let Some(waker) = source.waker.take() {
                waker.wake();
            }
        }
    }

    pub fn reset(&self) {
        let mut guard = self.source.lock();
        let source = guard.pinned().project();

        let new_deadline = Instant::now() + *source.period;
        *source.start_at = None;
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

        if !this.sleep.as_mut().poll(cx).is_ready() {
            return Poll::Pending;
        }

        // It hasn't been configured, so just ignore it.
        if *this.period == Duration::new(0, 0) && this.start_at.is_none() {
            return Poll::Pending;
        }

        *this.start_at = None;

        // Now reset the underlying timer.
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
