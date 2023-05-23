use std::{pin::Pin, task::Context};

use elfo_utils::{likely, unlikely};
use futures::Future;
use futures_intrusive::{
    buffer::GrowingHeapBuf,
    channel::{self, GenericChannel},
};
use parking_lot::{Mutex, RawMutex};
use tokio::pin;

use crate::{
    envelope::Envelope,
    errors::{SendError, TrySendError},
    tracing::TraceId,
};

// TODO: make mailboxes bounded by time instead of size.
const LIMIT: usize = 100_000;
const HIGH_PRIORITY_LIMIT: usize = 100;

pub(crate) struct Mailbox {
    queue: GenericChannel<RawMutex, Envelope, GrowingHeapBuf<Envelope>>,
    high_priority_queue: GenericChannel<RawMutex, Envelope, GrowingHeapBuf<Envelope>>,
    close: Mutex<Option<TraceId>>,
}

impl Mailbox {
    pub(crate) fn new() -> Self {
        Self {
            queue: GenericChannel::with_capacity(LIMIT),
            high_priority_queue: GenericChannel::with_capacity(HIGH_PRIORITY_LIMIT),
            close: Mutex::new(None),
        }
    }

    pub(crate) async fn send(&self, envelope: Envelope) -> Result<(), SendError<Envelope>> {
        let fut = self.queue.send(envelope);
        fut.await.map_err(|err| SendError(err.0))
    }

    pub(crate) async fn send_high_priority(
        &self,
        envelope: Envelope,
    ) -> Result<(), SendError<Envelope>> {
        let fut = self.high_priority_queue.send(envelope);
        fut.await.map_err(|err| SendError(err.0))
    }

    pub(crate) fn try_send(&self, envelope: Envelope) -> Result<(), TrySendError<Envelope>> {
        self.queue.try_send(envelope).map_err(|err| match err {
            channel::TrySendError::Full(envelope) => TrySendError::Full(envelope),
            channel::TrySendError::Closed(envelope) => TrySendError::Closed(envelope),
        })
    }

    pub(crate) fn try_send_high_priority(
        &self,
        envelope: Envelope,
    ) -> Result<(), TrySendError<Envelope>> {
        self.high_priority_queue
            .try_send(envelope)
            .map_err(|err| match err {
                channel::TrySendError::Full(envelope) => TrySendError::Full(envelope),
                channel::TrySendError::Closed(envelope) => TrySendError::Closed(envelope),
            })
    }

    pub(crate) async fn recv(&self) -> RecvResult {
        // let received = tokio::select! {
        //     high_priority_envelope = self.high_priority_queue.receive() =>
        // high_priority_envelope,     envelope = self.queue.receive() =>
        // envelope, };
        let mut high_priority_fut = self.high_priority_queue.receive();
        let mut regular_priority_fut = self.queue.receive();
        let received = std::future::poll_fn(|cx: &mut Context<'_>| {
            // SAFETY: future is stored on the stack above and never moved.
            let high_priority_pin = unsafe { Pin::new_unchecked(&mut high_priority_fut) };
            let regular_priority_pin = unsafe { Pin::new_unchecked(&mut regular_priority_fut) };

            let high_priority_poll = Future::poll(high_priority_pin, cx);
            if unlikely(high_priority_poll.is_ready()) {
                high_priority_poll
            } else {
                Future::poll(regular_priority_pin, cx)
            }
        })
        .await;

        match received {
            Some(envelope) => RecvResult::Data(envelope),
            None => self.on_close(),
        }
    }

    pub(crate) fn try_recv(&self) -> Option<RecvResult> {
        let high_priority = self.high_priority_queue.try_receive();
        if likely(matches!(
            high_priority,
            Err(channel::TryReceiveError::Empty)
        )) {
            return match self.queue.try_receive() {
                Ok(envelope) => Some(RecvResult::Data(envelope)),
                Err(channel::TryReceiveError::Empty) => None,
                Err(channel::TryReceiveError::Closed) => Some(self.on_close()),
            };
        } else if likely(high_priority.is_ok()) {
            return Some(RecvResult::Data(high_priority.unwrap()));
        } else
        // if unlikely(matches!(high_priority, Err(channel::TryReceiveError::Closed)))
        {
            return Some(self.on_close());
        }
    }

    #[cold]
    pub(crate) fn close(&self, trace_id: TraceId) -> bool {
        if self.queue.close().is_newly_closed() {
            *self.close.lock() = Some(trace_id);
            true
        } else {
            false
        }
    }

    #[cold]
    pub(crate) fn drop_all(&self) {
        while self.queue.try_receive().is_ok() {}
    }

    #[cold]
    fn on_close(&self) -> RecvResult {
        let trace_id = self.close.lock().expect("called before close()");
        RecvResult::Closed(trace_id)
    }
}

#[allow(clippy::large_enum_variant)]
pub(crate) enum RecvResult {
    Data(Envelope),
    Closed(TraceId),
}
