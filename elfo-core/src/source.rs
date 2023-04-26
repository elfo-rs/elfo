use derive_more::Constructor;
use sealed::sealed;

/// Note that implementations must be fused.
#[sealed(pub(crate))]
pub trait Source {
    // TODO: use `RecvResult` instead?
    #[doc(hidden)]
    fn poll_recv(&self, cx: &mut task::Context<'_>) -> Poll<Option<Envelope>>;

    // TODO: try_recv.
}

#[sealed]
impl<S: Source> Source for &S {
    #[inline]
    fn poll_recv(&self, cx: &mut task::Context<'_>) -> Poll<Option<Envelope>> {
        (**self).poll_recv(cx)
    }
}

#[sealed]
impl Source for () {
    #[inline]
    fn poll_recv(&self, _cx: &mut task::Context<'_>) -> Poll<Option<Envelope>> {
        // TODO: reconsider this.
        Poll::Pending
    }
}

#[sealed]
impl<S: Source> Source for Option<S> {
    #[inline]
    fn poll_recv(&self, cx: &mut task::Context<'_>) -> Poll<Option<Envelope>> {
        match self {
            Some(s) => s.poll_recv(cx),
            None => Poll::Pending,
        }
    }
}

#[derive(Constructor)]
pub struct Combined<L, R> {
    left: L,
    right: R,
}

#[sealed]
impl<L, R> Source for Combined<L, R>
where
    L: Source,
    R: Source,
{
    #[inline]
    fn poll_recv(&self, cx: &mut task::Context<'_>) -> Poll<Option<Envelope>> {
        match self.left.poll_recv(cx) {
            v @ Poll::Ready(Some(_)) => v,
            Poll::Ready(None) => self.right.poll_recv(cx),
            Poll::Pending => match self.right.poll_recv(cx) {
                v @ Poll::Ready(Some(_)) => v,
                _ => Poll::Pending,
            },
        }
    }
}

// NEW API

use std::{
    any::Any,
    marker::PhantomData,
    pin::Pin,
    ptr,
    sync::Arc,
    task::{self, Poll, RawWaker, RawWakerVTable, Waker},
};

use parking_lot::{Mutex, MutexGuard};
use unicycle::StreamsUnordered;

use crate::envelope::Envelope;

pub(crate) trait SourceStream: Send + 'static {
    fn as_any_mut(&mut self) -> &mut dyn Any;
    fn poll_recv(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<Option<Envelope>>;
}

pub struct Unattached<H> {
    source: UntypedSourceArc,
    handle: H,
}

impl<H> Unattached<H> {
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

// === SourceArc ===

pub(crate) struct SourceArc<S: ?Sized> {
    inner: UntypedSourceArc,
    marker: PhantomData<S>,
}

impl<S: ?Sized> SourceArc<S> {
    /// TODO
    pub(crate) fn from_untyped(inner: UntypedSourceArc) -> Self {
        let marker = PhantomData;
        Self { inner, marker }
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

// TODO: visibility.
#[derive(Clone)]
pub(crate) struct UntypedSourceArc(Arc<Mutex<StreamWithWaker<dyn SourceStream>>>);

struct StreamWithWaker<S: ?Sized> {
    waker: Waker,
    stream: S,
}

impl UntypedSourceArc {
    pub(crate) fn new(stream: impl SourceStream) -> Self {
        Self(Arc::new(Mutex::new(StreamWithWaker {
            waker: noop_waker(),
            stream,
        })))
    }
}

impl futures::Stream for UntypedSourceArc {
    type Item = Envelope;

    fn poll_next(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<Option<Envelope>> {
        let mut guard = self.0.lock();

        // SAFETY: The data inside the `Mutex` is pinned and we don't move it out here.
        let result = unsafe { Pin::new_unchecked(&mut guard.stream) }.poll_recv(cx);

        // Save the waker for reconfiguration if the stream isn't ready.
        // NOTE: `unicycle` doesn't support `will_wake` for now:
        // https://github.com/udoprog/unicycle/pull/15#issuecomment-1100680368
        // But we use it anyway to get benefits in the future.
        if result.is_pending() && !guard.waker.will_wake(cx.waker()) {
            guard.waker = cx.waker().clone();
        }

        result
    }
}

fn noop_waker() -> Waker {
    // SAFETY: it returns an object that upholds the right RawWaker contract.
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
