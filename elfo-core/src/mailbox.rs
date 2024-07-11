//! Mailboxes are MPSC channels for sending messages between actors.
//!
//! The current implementation is based on an intrusive linked list of envelopes
//! (using the `cordyceps` crate) and provides the following properties:
//! 1. Supports messages of different sizes.
//! 2. Supports both bounded and unbounded usage.
//! 3. The capacity is configurable on the fly.
//! 4. Preallocates no additional memory.
//!
//! A simplified structure can be pictured in the following way:
//! ```text
//!   mailbox                       envelopes
//! ┌─────────┐    ┌►┌───────┐    ┌►┌───────┐    ┌►┌───────┐◄─┐
//! │  head   ├────┘ │  lnk  ├────┘ │  lnk  ├────┘ │  lnk  │  │
//! ├─────────┤      ├───────┤      ├───────┤      ├───────┤  │
//! │  tail   ├─┐    │  hdr  │      │  hdr  │      │  hdr  │  │
//! ├─────────┤ │    ├───────┤      ├───────┤      ├───────┤  │
//! │ signals │ │    │       │      │  msg  │      │       │  │
//! └─────────┘ │    │       │      │   B   │      │  msg  │  │
//!             │    │  msg  │      └───────┘      │   C   │  │
//!             │    │   A   │                     │       │  │
//!             │    │       │                     └───────┘  │
//!             │    │       │                                │
//!             │    └───────┘                                │
//!             └─────────────────────────────────────────────┘
//! ```

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

assert_not_impl_any!(EnvelopeHeader: Unpin);

// SAFETY:
// * `EnvelopeHeader` is pinned in memory while it is in the queue, the only way
//   to access inserted `EnvelopeHeader` is by using the `dequeue()` method.
// * `EnvelopeHeader` cannot be deallocated without prunning the queue, which is
//   done also by calling `dequeue()` method multiple times.
// * `EnvelopeHeader` doesn't implement `Unpin` (checked statically above).
unsafe impl Linked<Link> for EnvelopeHeader {
    // It would be nice to enforce pinning here by using `Pin<Envelope>`.
    // However, it's not possible because `Pin` requires `Deref` impl.
    type Handle = Envelope;

    fn into_ptr(handle: Self::Handle) -> NonNull<Self> {
        handle.into_header_ptr()
    }

    unsafe fn from_ptr(ptr: NonNull<Self>) -> Self::Handle {
        Self::Handle::from_header_ptr(ptr)
    }

    unsafe fn links(ptr: NonNull<Self>) -> NonNull<Link> {
        // Using `ptr::addr_of_mut!` permits us to avoid creating a temporary
        // reference without using layout-dependent casts.
        let links = ptr::addr_of_mut!((*ptr.as_ptr()).link);

        // `NonNull::new_unchecked` is safe to use here, because the pointer that
        // we offset was not null, implying that the pointer produced by offsetting
        // it will also not be null.
        NonNull::new_unchecked(links)
    }
}

// TODO: make configurable (a config + `ctx.set_mailbox_capacity(_)`).
const LIMIT: usize = 100_000;

pub(crate) struct Mailbox {
    /// A storage for envelopes based on an intrusive linked list.
    /// Note: `cordyceps` uses terms "head" and "tail" in the opposite way.
    queue: MpscQueue<EnvelopeHeader>,

    /// A notifier of senders about the availability of new messages.
    // TODO: replace with a custom semaphore based on `async-event` (10-15% faster).
    tx_semaphore: Semaphore,

    /// A notifier of a receiver about the availability of new messages.
    // TODO: replace with `diatomic-waker` (3-5% faster).
    rx_notify: CachePadded<Notify>,

    /// A trace ID that should be assigned once the mailbox is closed.
    /// Use `Mutex` here for synchronization on close, more in `close()`.
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

    pub(crate) fn unbounded_send(&self, envelope: Envelope) -> Result<(), SendError<Envelope>> {
        if !self.tx_semaphore.is_closed() {
            self.queue.enqueue(envelope);
            self.rx_notify.notify_one();
            Ok(())
        } else {
            Err(SendError(envelope))
        }
    }

    pub(crate) async fn recv(&self) -> RecvResult {
        loop {
            // TODO: it should be possible to use `dequeue_unchecked()` here.
            // Preliminarily, we should guarantee that it can be called only
            // by one consumer. However, it's not enough to create a dedicated
            // `MailboxConsumer` because users can steal `Context` to another
            // task/thread and create a race with the `drop_all()` method.
            if let Some(envelope) = self.queue.dequeue() {
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

pub(crate) enum RecvResult {
    Data(Envelope),
    Closed(TraceId),
}
