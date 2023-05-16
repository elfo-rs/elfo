use std::{
    any::Any,
    marker::PhantomData,
    mem::ManuallyDrop,
    pin::Pin,
    ptr,
    task::{self, Poll, RawWaker, RawWakerVTable, Waker},
};

use sealed::sealed;
use unicycle::StreamsUnordered;

use self::pinarcmutex::{PinArcMutex, PinArcMutexGuard};
use crate::envelope::Envelope;

pub(crate) trait SourceStream: Send + 'static {
    fn as_any_mut(self: Pin<&mut Self>) -> Pin<&mut dyn Any>;
    fn poll_recv(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<Option<Envelope>>;
}

/// A wrapper to indicate that a source hasn't been attached to a context yet.
///
/// Sources does nothing unless they are attached to a context.
/// Use [`Context::attach()`] to do it.
///
/// [`Context::attach()`]: crate::context::Context::attach()
#[must_use = "sources do nothing unless you attach them"]
pub struct UnattachedSource<H> {
    source: UntypedSourceArc,
    handle: H,
}

impl<H> UnattachedSource<H> {
    pub(crate) fn new<S>(source: SourceArc<S>, handle: impl FnOnce(SourceArc<S>) -> H) -> Self
    where
        S: SourceStream + ?Sized,
    {
        Self {
            source: source.inner.to_owner(),
            handle: handle(source),
        }
    }

    pub(crate) fn attach_to(self, sources: &mut Sources) -> H {
        sources.push(self.source);
        self.handle
    }
}

// === SourceHandle ===

/// Defines common methods for sources.
#[sealed(pub(crate))]
pub trait SourceHandle {
    /// Returns `true` if the source has stopped producing messages.
    fn is_terminated(&self) -> bool;

    /// Terminates the source. `Drop` is called immediately.
    fn terminate(self);
}

// === SourceArc ===

pub(crate) struct SourceArc<S: ?Sized> {
    inner: UntypedSourceArc,
    marker: PhantomData<S>,
}

impl<S: ?Sized> SourceArc<S> {
    /// Creates a new `SourceArc` from an unsized `SourceStream`:
    /// `SourceArc::new()` cannot be used until `CoerceUnsized` is
    /// [stabilized](https://github.com/rust-lang/rust/issues/18598).
    pub(crate) fn from_untyped(inner: UntypedSourceArc) -> Self {
        let marker = PhantomData;
        Self { inner, marker }
    }
}

impl<S: SourceStream> SourceArc<S> {
    pub(crate) fn new(source: S, oneshot: bool) -> Self {
        Self::from_untyped(UntypedSourceArc::new(source, oneshot))
    }
}

impl<S: ?Sized> SourceArc<S> {
    /// Returns `None` if the source is terminated.
    pub(crate) fn lock(&self) -> Option<SourceStreamGuard<'_, S>> {
        let inner = self.inner.inner.lock();

        // If the source is terminated, we cannot give an access to it.
        // It also solves [the ABA problem in unicycle](https://github.com/udoprog/unicycle/issues/27),
        // but only for our own configuration methods.
        if inner.status() == StreamStatus::Terminated {
            return None;
        }

        Some(SourceStreamGuard {
            inner,
            marker: PhantomData,
        })
    }
}

// === SourceStreamGuard ===

pub(crate) struct SourceStreamGuard<'a, S: ?Sized> {
    inner: PinArcMutexGuard<'a, StreamWithWaker<dyn SourceStream>>,
    marker: PhantomData<S>,
}

impl<S: ?Sized> SourceStreamGuard<'_, S> {
    pub(crate) fn terminate(mut self) {
        self.inner.get_mut().terminate();

        // The method is called by a handle, so we should wake the stream
        // inside `unicycle` to poll it again and actually remove from the list.
        self.inner.wake();
    }

    pub(crate) fn wake(&self) {
        self.inner.wake();
    }
}

impl<S: 'static> SourceStreamGuard<'_, S> {
    pub(crate) fn stream(&mut self) -> Pin<&mut S> {
        let inner = self.inner.get_mut();
        let stream = inner.stream().as_any_mut();

        // SAFETY: we only downcast a reference here, it cannot move data.
        unsafe { stream.map_unchecked_mut(|s| s.downcast_mut::<S>().expect("invalid source type")) }
    }
}

// === UntypedSourceArc ===

pub(crate) struct UntypedSourceArc {
    /// `true` if it's an instance polled by `unicycle`.
    /// Used for checking in the `Drop` instance to terminate the source.
    is_owner: bool,
    inner: PinArcMutex<StreamWithWaker<dyn SourceStream>>,
}

impl UntypedSourceArc {
    pub(crate) fn new(stream: impl SourceStream, oneshot: bool) -> Self {
        Self {
            is_owner: false,
            inner: pinarcmutex::new!(StreamWithWaker {
                waker: noop_waker(),
                status: if oneshot {
                    StreamStatus::Oneshot
                } else {
                    StreamStatus::Stream
                },
                stream: ManuallyDrop::new(stream),
            }),
        }
    }

    fn to_owner(&self) -> Self {
        Self {
            is_owner: true,
            inner: self.inner.clone(),
        }
    }
}

impl Drop for UntypedSourceArc {
    fn drop(&mut self) {
        // If `unicycle` is being dropped (e.g. an actor is terminating), we should
        // terminate sources. We cannot rely on `Drop` of `StreamWithWaker` because
        // some handles may still exist (e.g. in another thread).
        if !self.is_owner {
            return;
        }

        let mut inner = self.inner.lock();
        if inner.status() != StreamStatus::Terminated {
            inner.get_mut().terminate();
        }
    }
}

struct StreamWithWaker<S: ?Sized> {
    waker: Waker,
    status: StreamStatus,
    // `stream` is considered pinned.
    stream: ManuallyDrop<S>,
}

/// Possible transitions:
/// * Stream → Terminated
/// * Oneshot → Terminated
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StreamStatus {
    Terminated,
    Stream,
    Oneshot,
}

impl<S: ?Sized> StreamWithWaker<S> {
    fn status(&self) -> StreamStatus {
        self.status
    }

    fn update_waker(self: Pin<&mut Self>, cx: &task::Context<'_>) {
        let new_waker = cx.waker();

        // Save the waker for reconfiguration if the stream isn't ready.
        // NOTE: `unicycle` doesn't support `will_wake` for now:
        // https://github.com/udoprog/unicycle/pull/15#issuecomment-1100680368
        // But we use it anyway to get benefits in the future.
        if !self.waker.will_wake(new_waker) {
            // SAFETY: `waker` is not pinned.
            unsafe { self.get_unchecked_mut().waker = new_waker.clone() }
        }
    }

    fn wake(&self) {
        self.waker.wake_by_ref();
    }

    fn stream(self: Pin<&mut Self>) -> Pin<&mut S> {
        assert_ne!(self.status, StreamStatus::Terminated);

        // SAFETY (`Pin`): `stream` is pinned when `Self` is.
        // SAFETY (`ManuallyDrop`): `Terminated` prevents double dropping.
        unsafe { self.map_unchecked_mut(|s| &mut *s.stream) }
    }

    fn terminate(self: Pin<&mut Self>) {
        assert_ne!(self.status, StreamStatus::Terminated);

        let this = unsafe { self.get_unchecked_mut() };
        this.status = StreamStatus::Terminated;

        // SAFETY (`Pin`): the destructor is called in-place without moving the stream.
        // SAFETY (`ManuallyDrop`): `Terminated` prevents double dropping.
        unsafe { ManuallyDrop::drop(&mut this.stream) };
    }
}

impl futures::Stream for UntypedSourceArc {
    type Item = Envelope;

    fn poll_next(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<Option<Envelope>> {
        let mut guard = self.inner.lock();

        // This is where we actually remove the stream from the list.
        if guard.status() == StreamStatus::Terminated {
            return Poll::Ready(None);
        }

        let result = guard.get_mut().stream().poll_recv(cx);

        if result.is_pending() {
            guard.get_mut().update_waker(cx);
        } else if matches!(result, Poll::Ready(None)) || guard.status() == StreamStatus::Oneshot {
            guard.get_mut().terminate();
        }

        result
    }
}

fn noop_waker() -> Waker {
    // SAFETY: it returns an object that upholds the right `RawWaker` contract.
    unsafe { Waker::from_raw(noop_raw_waker()) }
}

fn noop_raw_waker() -> RawWaker {
    fn noop_clone(_: *const ()) -> RawWaker {
        noop_raw_waker()
    }
    fn noop_wake(_: *const ()) {}
    fn noop_wake_by_ref(_: *const ()) {}
    fn noop_drop(_: *const ()) {}

    let vtable = &RawWakerVTable::new(noop_clone, noop_wake, noop_wake_by_ref, noop_drop);
    RawWaker::new(ptr::null(), vtable)
}

pub(crate) type Sources = StreamsUnordered<UntypedSourceArc>;

// === PinArcMutex ===

mod pinarcmutex {
    use std::{ops::Deref, pin::Pin, sync::Arc};

    use parking_lot::{Mutex, MutexGuard};

    /// To have a `?Sized` constructor until `CoerceUnsized` is stable.
    macro_rules! new {
        ($value:expr) => {
            pinarcmutex::PinArcMutex {
                __inner: std::sync::Arc::new(parking_lot::Mutex::new($value)),
            }
        };
    }
    pub(super) use new;

    pub(super) struct PinArcMutex<T: ?Sized> {
        /// Only for `new!`.
        pub(super) __inner: Arc<Mutex<T>>,
    }

    impl<T: ?Sized> PinArcMutex<T> {
        pub(super) fn lock(&self) -> PinArcMutexGuard<'_, T> {
            PinArcMutexGuard(self.__inner.lock())
        }
    }

    impl<T: ?Sized> Clone for PinArcMutex<T> {
        fn clone(&self) -> Self {
            Self {
                __inner: self.__inner.clone(),
            }
        }
    }

    pub(super) struct PinArcMutexGuard<'a, T: ?Sized>(MutexGuard<'a, T>);

    impl<T: ?Sized> PinArcMutexGuard<'_, T> {
        pub(super) fn get_mut(&mut self) -> Pin<&mut T> {
            // SAFETY: there is no way to get also `&mut T`.
            unsafe { Pin::new_unchecked(&mut *self.0) }
        }
    }

    impl<T: ?Sized> Deref for PinArcMutexGuard<'_, T> {
        type Target = T;

        fn deref(&self) -> &Self::Target {
            // See https://doc.rust-lang.org/stable/std/pin/struct.Pin.html#method.get_ref
            // for details why this is can be considered safe.
            &self.0
        }
    }
}
