use std::{
    future::Future,
    pin::Pin,
    task::{self, Poll},
};

use parking_lot::Mutex;
use sealed::sealed;
use tokio::time::{self, Duration, Instant, Sleep};

use crate::{
    addr::Addr,
    envelope::{Envelope, MessageKind},
    message::Message,
    tracing::TraceId,
};

/// `Source` that produces a message after a scheduled duration.
///
/// Does nothing until scheduled.
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

#[sealed]
impl<M, F> crate::source::Source for Stopwatch<F>
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
            let trace_id = TraceId::generate();
            let envelope = Envelope::with_trace_id(message, kind, trace_id).upcast();
            return Poll::Ready(Some(envelope));
        }

        Poll::Pending
    }
}

#[cfg(test)]
#[cfg(feature = "test-util")]
mod tests {
    use super::*;

    use futures::{future::poll_fn, poll};
    use tokio::time;

    use elfo_macros::message;

    use crate::source::Source;

    #[message(elfo = crate)]
    struct Timeout;

    #[tokio::test]
    async fn it_works() {
        time::pause();

        let sw = Stopwatch::new(|| Timeout);

        for _ in 0..=5 {
            // Before scheduling.
            let res = poll!(poll_fn(|cx| sw.poll_recv(cx)));
            assert!(res.is_pending());

            sw.schedule_after(Duration::from_secs(10));
            let res = poll!(poll_fn(|cx| sw.poll_recv(cx)));
            assert!(res.is_pending());

            // Some time passed, but not enough yet.
            time::advance(Duration::from_secs(9)).await;
            let res = poll!(poll_fn(|cx| sw.poll_recv(cx)));
            assert!(res.is_pending());

            // Time passed.
            time::advance(Duration::from_millis(1001)).await;
            let res = poll!(poll_fn(|cx| sw.poll_recv(cx)));
            assert!(res.is_ready());

            // Fired only once.
            let res = poll!(poll_fn(|cx| sw.poll_recv(cx)));
            assert!(res.is_pending());
        }
    }
}
