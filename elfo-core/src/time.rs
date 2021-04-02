use std::{
    future::Future,
    pin::Pin,
    task::{self, Poll},
};

use parking_lot::Mutex;
use tokio::time::{self, Duration, Instant, Sleep};

use crate::{
    addr::Addr,
    context::Source,
    envelope::{Envelope, MessageKind},
    message::Message,
};

pub struct Interval<F> {
    message_factory: F,
    state: Mutex<State>,
}

struct State {
    sleep: Pin<Box<Sleep>>,
    start_at: Option<Instant>,
    period: Duration,
}

impl<F> Interval<F> {
    pub fn new(f: F) -> Self {
        Self {
            message_factory: f,
            state: Mutex::new(State {
                sleep: Box::pin(time::sleep_until(Instant::now())),
                start_at: None,
                period: Duration::new(0, 0),
            }),
        }
    }

    pub fn after(self, after: Duration) -> Self {
        let when = Instant::now() + after;
        let mut state = self.state.lock();
        state.start_at = Some(when);
        state.sleep.as_mut().reset(when);
        drop(state);
        self
    }

    pub fn set_period(&self, new_period: Duration) {
        assert!(new_period > Duration::new(0, 0));

        let mut state = self.state.lock();

        if new_period == state.period {
            return;
        }

        let old_period = state.period;
        state.period = new_period;

        if state.start_at.is_none() {
            let new_deadline = state.sleep.deadline() - old_period + new_period;
            state.sleep.as_mut().reset(new_deadline);
        }
    }

    pub fn reset(&self) {
        let mut state = self.state.lock();
        let new_deadline = Instant::now() + state.period;
        state.start_at = None;
        state.sleep.as_mut().reset(new_deadline);
    }
}

impl<M, F> Source for Interval<F>
where
    F: Fn() -> M,
    M: Message,
{
    fn poll_recv(&self, cx: &mut task::Context<'_>) -> Poll<Option<Envelope>> {
        let mut state = self.state.lock();

        if state.sleep.as_mut().poll(cx).is_ready() {
            // It hasn't been configured, so just ignore it.
            if state.period == Duration::new(0, 0) && state.start_at.is_none() {
                return Poll::Pending;
            }

            state.start_at = None;

            // Now reset the underlying timer.
            let period = state.period;
            let sleep = state.sleep.as_mut();
            let new_deadline = sleep.deadline() + period;
            sleep.reset(new_deadline);

            // Emit a message.
            let message = (self.message_factory)();
            let kind = MessageKind::Regular { sender: Addr::NULL };
            let envelope = Envelope::new(message, kind).upcast();
            return Poll::Ready(Some(envelope));
        }

        Poll::Pending
    }
}
