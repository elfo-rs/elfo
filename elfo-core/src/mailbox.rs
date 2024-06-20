use std::ptr::{self, NonNull};

use cordyceps::{
    mpsc_queue::{Links, MpscQueue},
    Linked,
};
use parking_lot::Mutex;
use tokio::sync::{Notify, Semaphore, TryAcquireError};

use elfo_utils::CachePadded;

use crate::{
    envelope::{Envelope, EnvelopeHeader},
    errors::{SendError, TrySendError},
    tracing::TraceId,
};

pub(crate) type Link = Links<EnvelopeHeader>;

unsafe impl Linked<Links<EnvelopeHeader>> for EnvelopeHeader {
    type Handle = Envelope;

    // TODO: Pin?

    fn into_ptr(handle: Self::Handle) -> NonNull<Self> {
        handle.into_header_ptr()
    }

    /// Convert a raw pointer back into an owned `Handle`.
    unsafe fn from_ptr(ptr: NonNull<Self>) -> Self::Handle {
        unsafe { Self::Handle::from_header_ptr(ptr) }
    }

    /// Access an element's `Links`.
    unsafe fn links(ptr: NonNull<Self>) -> NonNull<Links<Self>> {
        // Using `ptr::addr_of_mut!` permits us to avoid creating a temporary
        // reference without using layout-dependent casts.
        let links = ptr::addr_of_mut!((*ptr.as_ptr()).link);

        // `NonNull::new_unchecked` is safe to use here, because the pointer that
        // we offset was not null, implying that the pointer produced by offsetting
        // it will also not be null.
        NonNull::new_unchecked(links)
    }
}

// TODO: make configurable && limit by time.
const LIMIT: usize = 100_000;

pub(crate) struct Mailbox {
    queue: MpscQueue<EnvelopeHeader>,
    tx_semaphore: Semaphore,
    rx_notify: CachePadded<Notify>,
    closed_trace_id: Mutex<Option<TraceId>>,
}

impl Mailbox {
    pub(crate) fn new() -> Self {
        Self {
            queue: MpscQueue::new_with_stub(Envelope::stub()),
            tx_semaphore: Semaphore::new(LIMIT),
            rx_notify: CachePadded::new(Notify::new()),
            closed_trace_id: Mutex::new(None),
        }
    }

    pub(crate) async fn send(&self, envelope: Envelope) -> Result<(), SendError<Envelope>> {
        let permit = match self.tx_semaphore.acquire().await {
            Ok(permit) => permit,
            Err(_) => return Err(SendError(envelope)),
        };

        permit.forget();
        self.queue.enqueue(envelope);
        self.rx_notify.notify_one();
        Ok(())
    }

    pub(crate) fn try_send(&self, envelope: Envelope) -> Result<(), TrySendError<Envelope>> {
        match self.tx_semaphore.try_acquire() {
            Ok(permit) => {
                permit.forget();
                self.queue.enqueue(envelope);
                self.rx_notify.notify_one();
                Ok(())
            }
            Err(TryAcquireError::NoPermits) => Err(TrySendError::Full(envelope)),
            Err(TryAcquireError::Closed) => Err(TrySendError::Closed(envelope)),
        }
    }

    pub(crate) async fn recv(&self) -> RecvResult {
        loop {
            if let Some(envelope) = self.queue.dequeue() {
                // TODO: try_dequeue?
                self.tx_semaphore.add_permits(1);
                return RecvResult::Data(envelope);
            }

            if self.tx_semaphore.is_closed() {
                return self.on_close();
            }

            self.rx_notify.notified().await;
        }
    }

    pub(crate) fn try_recv(&self) -> Option<RecvResult> {
        match self.queue.dequeue() {
            Some(envelope) => {
                self.tx_semaphore.add_permits(1);
                Some(RecvResult::Data(envelope))
            }
            None if self.tx_semaphore.is_closed() => Some(self.on_close()),
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

        if self.tx_semaphore.is_closed() {
            return false;
        }

        *closed_trace_id = Some(trace_id);

        self.tx_semaphore.close();
        self.rx_notify.notify_one();
        true
    }

    #[cold]
    pub(crate) fn drop_all(&self) {
        while self.queue.dequeue().is_some() {}
    }

    #[cold]
    fn on_close(&self) -> RecvResult {
        // Some messages may be in the queue after the channel is closed.
        match self.queue.dequeue() {
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
