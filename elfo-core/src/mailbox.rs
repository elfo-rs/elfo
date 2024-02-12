use std::sync::atomic::{AtomicBool, Ordering};

use crossbeam_queue::SegQueue;
use futures_intrusive::sync::GenericSemaphore;
use parking_lot::{Mutex, RawMutex};
use tokio::sync::Notify;

use elfo_utils::CachePadded;

use crate::{
    envelope::Envelope,
    errors::{SendError, TrySendError},
    tracing::TraceId,
};

// TODO: make mailboxes bounded by time instead of size.
const LIMIT: usize = 100_000;

pub(crate) struct Mailbox {
    queue: SegQueue<Envelope>,
    tx_semaphore: GenericSemaphore<RawMutex>,
    rx_notify: CachePadded<Notify>,
    is_closed: AtomicBool,
    closed_trace_id: Mutex<Option<TraceId>>,
}

impl Mailbox {
    pub(crate) fn new() -> Self {
        Self {
            queue: SegQueue::new(),
            tx_semaphore: GenericSemaphore::new(true, LIMIT),
            rx_notify: CachePadded(Notify::new()),
            is_closed: AtomicBool::new(false),
            closed_trace_id: Mutex::new(None),
        }
    }

    pub(crate) async fn send(&self, envelope: Envelope) -> Result<(), SendError<Envelope>> {
        let mut permit = self.tx_semaphore.acquire(1).await;

        if self.is_closed.load(Ordering::Relaxed) {
            return Err(SendError(envelope));
        }

        permit.disarm();
        self.queue.push(envelope);
        self.rx_notify.notify_one();
        Ok(())
    }

    pub(crate) fn try_send(&self, envelope: Envelope) -> Result<(), TrySendError<Envelope>> {
        match self.tx_semaphore.try_acquire(1) {
            Some(mut permit) => {
                if self.is_closed.load(Ordering::Relaxed) {
                    return Err(TrySendError::Closed(envelope));
                }

                permit.disarm();
                self.queue.push(envelope);
                self.rx_notify.notify_one();
                Ok(())
            }
            None => Err(TrySendError::Full(envelope)),
        }
    }

    pub(crate) async fn recv(&self) -> RecvResult {
        loop {
            if let Some(envelope) = self.queue.pop() {
                self.tx_semaphore.release(1);
                return RecvResult::Data(envelope);
            }

            if self.is_closed.load(Ordering::Relaxed) {
                return self.on_close();
            }

            self.rx_notify.notified().await;
        }
    }

    pub(crate) fn try_recv(&self) -> Option<RecvResult> {
        match self.queue.pop() {
            Some(envelope) => {
                self.tx_semaphore.release(1);
                Some(RecvResult::Data(envelope))
            }
            None if self.is_closed.load(Ordering::Relaxed) => Some(self.on_close()),
            None => None,
        }
    }

    #[cold]
    pub(crate) fn close(&self, trace_id: TraceId) -> bool {
        // NOTE: It is important that we take the lock here before actually closing the
        // channel. If we take a lock after closing the channel, data race is
        // possible when we try to `recv()` after the channel is closed, but
        // before the `closed_trace_id` is assigned.
        let mut closed_trace_id = self.closed_trace_id.lock();

        if self.is_closed.load(Ordering::Relaxed) {
            return false;
        }

        *closed_trace_id = Some(trace_id);
        self.is_closed.store(true, Ordering::Relaxed);
        self.rx_notify.notify_one();
        true
    }

    #[cold]
    pub(crate) fn drop_all(&self) {
        while self.queue.pop().is_some() {}
    }

    #[cold]
    fn on_close(&self) -> RecvResult {
        // Some messages may be in the queue after the channel is closed.
        match self.queue.pop() {
            Some(envelope) => RecvResult::Data(envelope),
            None => {
                let trace_id = self.closed_trace_id.lock().expect("called before close()");
                RecvResult::Closed(trace_id)
            }
        }
    }
}

#[allow(clippy::large_enum_variant)]
pub(crate) enum RecvResult {
    Data(Envelope),
    Closed(TraceId),
}
