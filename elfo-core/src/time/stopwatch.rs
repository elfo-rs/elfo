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
    trace_id,
};

/// `Source` that produces a message after a scheduled duration.
///
/// Do nothing until scheduled.
pub struct Stopwatch<F> {
    message_factory: F,
    state: Mutex<State>,
}

struct State {
    sleep: Pin<Box<Sleep>>,
    should_fire: bool,
}

impl<F> Stopwatch<F> {
    /// Creates a new `Stopwatch`.
    pub fn new(f: F) -> Self {
        Self {
            message_factory: f,
            state: Mutex::new(State {
                sleep: Box::pin(time::sleep_until(Instant::now())),
                should_fire: false,
            }),
        }
    }

    /// Produces a message when `deadline` is reached.
    #[inline]
    pub fn schedule_at(&self, deadline: Instant) {
        let mut state = self.state.lock();
        state.should_fire = true;
        state.sleep.as_mut().reset(deadline);
    }

    /// Produces a message once `duration` has elapsed.
    #[inline]
    pub fn schedule_after(&self, duration: Duration) {
        self.schedule_at(Instant::now() + duration)
    }
}

impl<M, F> Source for Stopwatch<F>
where
    F: Fn() -> M,
    M: Message,
{
    fn poll_recv(&self, cx: &mut task::Context<'_>) -> Poll<Option<Envelope>> {
        let mut state = self.state.lock();

        if state.should_fire && state.sleep.as_mut().poll(cx).is_ready() {
            state.should_fire = false;

            // Emit a message.
            let message = (self.message_factory)();
            let kind = MessageKind::Regular { sender: Addr::NULL };
            let trace_id = trace_id::generate();
            let envelope = Envelope::with_trace_id(message, kind, trace_id).upcast();
            return Poll::Ready(Some(envelope));
        }

        Poll::Pending
    }
}
