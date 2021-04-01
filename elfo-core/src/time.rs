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
    state: Mutex<Option<State>>,
}

struct State {
    sleep: Pin<Box<Sleep>>,
    period: Duration,
}

impl<F> Interval<F> {
    pub fn new(f: F) -> Self {
        Self {
            message_factory: f,
            state: Mutex::new(None),
        }
    }

    #[inline]
    pub fn set_period(&self, period: Duration) {
        let mut state = self.state.lock();

        if let Some(state) = state.as_mut() {
            if period != state.period {
                state.do_set_period(period);
            }
        } else {
            let sleep = time::sleep(period);
            *state = Some(State {
                sleep: Box::pin(sleep),
                period,
            });
        }
    }

    pub fn reset(&self) {
        let mut state = self.state.lock();

        if let Some(state) = state.as_mut() {
            let new_deadline = Instant::now() + state.period;
            state.sleep.as_mut().reset(new_deadline);
        }
    }
}

impl State {
    #[cold]
    fn do_set_period(&mut self, period: Duration) {
        let sleep = self.sleep.as_mut();
        let new_deadline = sleep.deadline() - self.period + period;
        self.period = period;
        sleep.reset(new_deadline);
    }
}

impl<M, F> Source for Interval<F>
where
    F: Fn() -> M,
    M: Message,
{
    fn poll_recv(&self, cx: &mut task::Context<'_>) -> Poll<Option<Envelope>> {
        let mut state = self.state.lock();

        if let Some(state) = state.as_mut() {
            if state.sleep.as_mut().poll(cx).is_ready() {
                let sleep = state.sleep.as_mut();
                let new_deadline = sleep.deadline() + state.period;
                sleep.reset(new_deadline);

                let message = (self.message_factory)();
                let kind = MessageKind::Regular { sender: Addr::NULL };
                let envelope = Envelope::new(message, kind).upcast();
                return Poll::Ready(Some(envelope));
            }
        }

        Poll::Pending
    }
}
