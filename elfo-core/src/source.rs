use std::{
    any::Any,
    marker::PhantomData,
    pin::Pin,
    ptr,
    sync::Arc,
    task::{self, Poll, RawWaker, RawWakerVTable, Waker},
};

use parking_lot::{Mutex, MutexGuard};
use sealed::sealed;
use unicycle::StreamsUnordered;

use crate::envelope::Envelope;

pub(crate) trait SourceStream: Send + 'static {
    fn as_any_mut(&mut self) -> &mut dyn Any;
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
    pub(crate) fn new(source: SourceArc<impl SourceStream + ?Sized>, handle: H) -> Self {
        Self {
            source: source.inner,
            handle,
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

    /// Terminates the source.
    ///
    /// The `Drop` will be called only on one of the following `(try_)recv()`.
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

impl<S: ?Sized> SourceArc<S> {
    pub(crate) fn is_terminated(&self) -> bool {
        // TODO: deadlock if `is_terminated()` is called inside `poll_recv()`.
        //       consider using `ReentrantMutex` or split the mutex instead.
        self.inner.0.lock().is_terminated
    }

    pub(crate) fn terminate(&self) {
        // TODO: deadlock if `terminate()` is called inside `poll_recv()`.
        //       consider using `ReentrantMutex` or split the mutex instead.
        let mut inner = self.inner.0.lock();

        if inner.is_terminated {
            return;
        }

        inner.is_terminated = true;
        inner.waker.wake_by_ref();
    }
}

impl<S: SourceStream> SourceArc<S> {
    pub(crate) fn new(source: S) -> Self {
        Self::from_untyped(UntypedSourceArc::new(source))
    }

    pub(crate) fn lock(&self) -> SourceStreamGuard<'_, S> {
        SourceStreamGuard {
            inner: self.inner.0.lock(),
            marker: PhantomData,
        }
    }
}

impl<S: ?Sized> Clone for SourceArc<S> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            marker: PhantomData,
        }
    }
}

// === SourceStreamGuard ===

pub(crate) struct SourceStreamGuard<'a, S: ?Sized> {
    inner: MutexGuard<'a, StreamWithWaker<dyn SourceStream>>,
    marker: PhantomData<S>,
}

impl<S: 'static> SourceStreamGuard<'_, S> {
    pub(crate) fn wake(&mut self) {
        // If the source is terminated, we should not wake it up.
        // It solves [the ABA problem in unicycle](https://github.com/udoprog/unicycle/issues/27),
        // but only for our own configuration methods.
        if self.inner.is_terminated {
            return;
        }

        self.inner.waker.wake_by_ref();
    }

    pub(crate) fn pinned(&mut self) -> Pin<&mut S> {
        let stream = self
            .inner
            .stream
            .as_any_mut()
            .downcast_mut::<S>()
            .expect("invalid source type");

        // SAFETY: The data inside the `Mutex` is pinned and we don't move it out here.
        unsafe { Pin::new_unchecked(stream) }
    }
}

// === UntypedSourceArc ===

#[derive(Clone)]
pub(crate) struct UntypedSourceArc(Arc<Mutex<StreamWithWaker<dyn SourceStream>>>);

struct StreamWithWaker<S: ?Sized> {
    waker: Waker,
    is_terminated: bool,
    stream: S,
}

impl UntypedSourceArc {
    pub(crate) fn new(stream: impl SourceStream) -> Self {
        Self(Arc::new(Mutex::new(StreamWithWaker {
            waker: noop_waker(),
            is_terminated: false,
            stream,
        })))
    }
}

impl futures::Stream for UntypedSourceArc {
    type Item = Envelope;

    fn poll_next(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<Option<Envelope>> {
        let mut guard = self.0.lock();

        if guard.is_terminated {
            return Poll::Ready(None);
        }

        // SAFETY: The data inside the `Mutex` is pinned and we don't move it out here.
        let result = unsafe { Pin::new_unchecked(&mut guard.stream) }.poll_recv(cx);

        // Save the waker for reconfiguration if the stream isn't ready.
        // NOTE: `unicycle` doesn't support `will_wake` for now:
        // https://github.com/udoprog/unicycle/pull/15#issuecomment-1100680368
        // But we use it anyway to get benefits in the future.
        if result.is_pending() && !guard.waker.will_wake(cx.waker()) {
            guard.waker = cx.waker().clone();
        }

        if matches!(result, Poll::Ready(None)) {
            guard.is_terminated = true;
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
