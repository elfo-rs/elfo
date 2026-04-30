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

use std::{
    future::poll_fn,
    mem,
    ops::Deref,
    ptr::{self, NonNull},
    task::Poll,
};

use cordyceps::{
    Linked,
    mpsc_queue::{Links, MpscQueue},
};
use derive_more::IsVariant;
use diatomic_waker::DiatomicWaker;
use parking_lot::{Mutex, MutexGuard};
use tokio::sync::{Semaphore, TryAcquireError};

use elfo_utils::CachePadded;

use crate::{
    envelope::{Envelope, EnvelopeHeader},
    errors::{SendError, TrySendError},
    tracing::TraceId,
};

// === MailboxConfig ===

pub mod config {
    //! [Config]
    //!
    //! [Config]: MailboxConfig

    /// Mailbox configuration.
    ///
    /// # Example
    /// ```toml
    /// [some_group]
    /// system.mailbox.capacity = 1000
    /// ```
    #[derive(Debug, PartialEq, serde::Deserialize)]
    #[serde(default)]
    pub struct MailboxConfig {
        /// The maximum number of messages that can be stored in the mailbox.
        ///
        /// Can be overriden by actor using [`Context::set_mailbox_capacity()`].
        ///
        /// `100` by default.
        ///
        /// [`Context::set_mailbox_capacity()`]: crate::Context::set_mailbox_capacity
        pub capacity: usize,
    }

    impl Default for MailboxConfig {
        fn default() -> Self {
            Self { capacity: 100 }
        }
    }
}

// === Mailbox ===

pub(crate) type Link = Links<EnvelopeHeader>;

assert_not_impl_any!(EnvelopeHeader: Unpin);

// SAFETY:
// * `EnvelopeHeader` is pinned in memory while it is in the queue, the only way
//   to access inserted `EnvelopeHeader` is by using the `dequeue_unchecked()`
//   method.
// * `EnvelopeHeader` cannot be deallocated without prunning the queue, which is
//   done also by calling `dequeue_unchecked()` method multiple times.
// * `EnvelopeHeader` doesn't implement `Unpin` (checked statically above).
unsafe impl Linked<Link> for EnvelopeHeader {
    // It would be nice to enforce pinning here by using `Pin<Envelope>`.
    // However, it's not possible because `Pin` requires `Deref` impl.
    type Handle = Envelope;

    fn into_ptr(handle: Self::Handle) -> NonNull<Self> {
        handle.into_header_ptr()
    }

    unsafe fn from_ptr(ptr: NonNull<Self>) -> Self::Handle {
        // SAFETY: `ptr` was produced by `into_ptr`, which wraps a valid `Envelope`.
        unsafe { Self::Handle::from_header_ptr(ptr) }
    }

    unsafe fn links(ptr: NonNull<Self>) -> NonNull<Link> {
        // Using `ptr::addr_of_mut!` permits us to avoid creating a temporary
        // reference without using layout-dependent casts.
        // SAFETY: `ptr` is valid for reads and points to a properly initialized
        // `EnvelopeHeader`.
        let links = unsafe { ptr::addr_of_mut!((*ptr.as_ptr()).link) };

        // SAFETY: `NonNull::new_unchecked` is safe to use here, because the pointer
        // that we offset was not null, implying that the pointer produced by offsetting
        // it will also not be null.
        unsafe { NonNull::new_unchecked(links) }
    }
}

pub(crate) struct Mailbox {
    /// A storage for envelopes based on an intrusive linked list.
    /// Note: `cordyceps` uses terms "head" and "tail" in the opposite way.
    queue: MpscQueue<EnvelopeHeader>,

    /// A notifier of senders about the availability of new messages.
    // TODO: replace with a custom semaphore based on `async-event` (10-15% faster).
    tx_semaphore: Semaphore,

    /// Wakes the consumer when a new envelope is enqueued or the
    /// mailbox is closed.
    rx_waker: CachePadded<DiatomicWaker>,

    /// Use `Mutex` here for synchronization on close/configure.
    control: Mutex<Control>,
}

struct Control {
    /// A trace ID that should be assigned once the mailbox is closed.
    closed_trace_id: Option<TraceId>,
    /// A real capacity of the mailbox.
    capacity: usize,
    /// State of the single consumer slot.
    consumer: ConsumerSlot,
}

/// State of the single consumer slot of a [`Mailbox`].
#[derive(IsVariant)]
enum ConsumerSlot {
    Vacant,
    Occupied { drain_on_drop: bool },
}

impl Mailbox {
    pub(crate) fn new(config: &config::MailboxConfig) -> Self {
        let capacity = clamp_capacity(config.capacity);

        Self {
            queue: MpscQueue::new_with_stub(Envelope::stub()),
            tx_semaphore: Semaphore::new(capacity),
            rx_waker: CachePadded::new(DiatomicWaker::new()),
            control: Mutex::new(Control {
                closed_trace_id: None,
                capacity,
                consumer: ConsumerSlot::Vacant,
            }),
        }
    }

    pub(crate) fn set_capacity(&self, capacity: usize) {
        let mut control = self.control.lock();

        if capacity == control.capacity {
            return;
        }

        if capacity < control.capacity {
            let delta = control.capacity - capacity;
            let real_delta = self.tx_semaphore.forget_permits(delta);

            // Note that we cannot reduce the number of active permits
            // (relates to messages that already stored in the queue) in tokio impl.
            // Sadly, in such cases, we violate provided `capacity`.
            debug_assert!(real_delta <= delta);
            control.capacity -= real_delta;
        } else {
            let real_delta = clamp_capacity(capacity) - control.capacity;
            self.tx_semaphore.add_permits(real_delta);
            control.capacity += real_delta;
        }
    }

    pub(crate) async fn send(&self, envelope: Envelope) -> Result<(), SendError<Envelope>> {
        let permit = match self.tx_semaphore.acquire().await {
            Ok(permit) => permit,
            Err(_) => return Err(SendError(envelope)),
        };

        permit.forget();
        self.queue.enqueue(envelope);
        self.rx_waker.notify();
        Ok(())
    }

    pub(crate) fn try_send(&self, envelope: Envelope) -> Result<(), TrySendError<Envelope>> {
        match self.tx_semaphore.try_acquire() {
            Ok(permit) => {
                permit.forget();
                self.queue.enqueue(envelope);
                self.rx_waker.notify();
                Ok(())
            }
            Err(TryAcquireError::NoPermits) => Err(TrySendError::Full(envelope)),
            Err(TryAcquireError::Closed) => Err(TrySendError::Closed(envelope)),
        }
    }

    pub(crate) fn unbounded_send(&self, envelope: Envelope) -> Result<(), SendError<Envelope>> {
        // NOTE: see `recv` below. Every `recv` add 1 permit even if send was unbounded,
        // thus, as an effect, mailbox's capacity gets larger for everyone every
        // time we do unbounded send, so we do this to mitigate a problem a bit
        // before more proper solution.
        //
        // TODO: instead semaphore should support loaning the permits.
        match self.tx_semaphore.try_acquire() {
            Ok(permit) => {
                permit.forget();
            }
            Err(TryAcquireError::Closed) => return Err(SendError(envelope)),
            Err(TryAcquireError::NoPermits) => {}
        }

        self.queue.enqueue(envelope);
        self.rx_waker.notify();

        Ok(())
    }

    #[cold]
    pub(crate) fn close(&self, trace_id: TraceId) -> bool {
        // NOTE: It is important that we take the lock here before actually closing the
        // channel. If we take a lock after closing the channel, data race is
        // possible when we try to `recv()` after the channel is closed, but
        // before the `closed_trace_id` is assigned.
        let mut control = self.control.lock();
        self.close_inner(&mut control, trace_id)
    }

    /// Closes the mailbox. Drains the queue inline if no consumer is attached
    /// (returns `true`); otherwise defers the drain to the consumer's detach
    /// (returns `false`).
    #[cold]
    pub(crate) fn close_and_try_drain(&self, trace_id: TraceId) -> bool {
        let mut control = self.control.lock();
        self.close_inner(&mut control, trace_id);
        if control.consumer.is_vacant() {
            // SAFETY: No other consumer is exist at this time.
            unsafe { while self.queue.dequeue_unchecked().is_some() {} }
            return true;
        }
        control.consumer = ConsumerSlot::Occupied {
            drain_on_drop: true,
        };
        false
    }

    fn close_inner(&self, control: &mut MutexGuard<'_, Control>, trace_id: TraceId) -> bool {
        if self.tx_semaphore.is_closed() {
            return false;
        }

        control.closed_trace_id = Some(trace_id);

        self.tx_semaphore.close();
        self.rx_waker.notify();
        true
    }
}

// === MailboxConsumer ===

/// The unique consumer half of a [`Mailbox`].
///
/// Single-consumer is enforced at runtime via `Control::consumer`.
pub(crate) struct MailboxConsumer<D: Deref<Target = Mailbox>>(D);

impl<D: Deref<Target = Mailbox>> MailboxConsumer<D> {
    /// Panics if a `MailboxConsumer` is already attached to `inner`.
    pub(crate) fn new(inner: D) -> Self {
        let mut control = inner.control.lock();
        assert!(
            control.consumer.is_vacant(),
            "a `MailboxConsumer` is already attached to this mailbox"
        );
        control.consumer = ConsumerSlot::Occupied {
            drain_on_drop: false,
        };
        drop(control);
        Self(inner)
    }

    pub(crate) async fn recv(&mut self) -> RecvResult {
        poll_fn(|cx| {
            if let Some(result) = self.try_recv() {
                return Poll::Ready(result);
            }
            // SAFETY: `MailboxConsumer` is the sole sink of `rx_waker`.
            unsafe { self.0.rx_waker.register(cx.waker()) };
            // Recheck to avoid a lost wake-up between the first `try_recv`
            // and our `register`.
            match self.try_recv() {
                Some(result) => {
                    // SAFETY: see `register` above.
                    unsafe { self.0.rx_waker.unregister() };
                    Poll::Ready(result)
                }
                None => Poll::Pending,
            }
        })
        .await
    }

    pub(crate) fn try_recv(&mut self) -> Option<RecvResult> {
        // SAFETY: sole consumer invariant — see [`MailboxConsumer`].
        match unsafe { self.0.queue.dequeue_unchecked() } {
            Some(envelope) => {
                self.0.tx_semaphore.add_permits(1);
                Some(RecvResult::Data(envelope))
            }
            None if self.0.tx_semaphore.is_closed() => Some(self.on_close()),
            None => None,
        }
    }

    #[cold]
    fn on_close(&self) -> RecvResult {
        // Some messages may be in the queue after the channel is closed.
        // SAFETY: sole consumer invariant — see [`MailboxConsumer`].
        match unsafe { self.0.queue.dequeue_unchecked() } {
            Some(envelope) => RecvResult::Data(envelope),
            None => {
                let control = self.0.control.lock();
                let trace_id = control.closed_trace_id.expect("called before close()");
                RecvResult::Closed(trace_id)
            }
        }
    }
}

impl<D: Deref<Target = Mailbox>> Drop for MailboxConsumer<D> {
    fn drop(&mut self) {
        let mut control = self.0.control.lock();
        let slot = mem::replace(&mut control.consumer, ConsumerSlot::Vacant);
        if let ConsumerSlot::Occupied {
            drain_on_drop: true,
        } = slot
        {
            // SAFETY: No other consumer is exist at this time.
            unsafe { while self.0.queue.dequeue_unchecked().is_some() {} }
        }
    }
}

pub(crate) enum RecvResult {
    Data(Envelope),
    Closed(TraceId),
}

fn clamp_capacity(capacity: usize) -> usize {
    capacity.min(Semaphore::MAX_PERMITS)
}
