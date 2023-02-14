use std::task::{self, Poll};

use derive_more::Constructor;
use sealed::sealed;

use crate::envelope::Envelope;

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

use std::{any::Any, marker::PhantomData, pin::Pin, sync::Arc};

use parking_lot::{Mutex, MutexGuard};
use unicycle::StreamsUnordered;

pub(crate) trait SourceStream: Send + 'static {
    fn as_any_mut(&mut self) -> &mut dyn Any;
    fn poll_recv(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<Option<Envelope>>;
}

pub struct Unattached<H> {
    source: UntypedSourceArc,
    handle: H,
}

impl<H> Unattached<H> {
    pub(crate) fn new(source: SourceArc<impl SourceStream>, handle: H) -> Self {
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

pub(crate) struct SourceArc<S> {
    inner: UntypedSourceArc,
    marker: PhantomData<S>,
}

impl<S: SourceStream> SourceArc<S> {
    pub(crate) fn new(source: S) -> Self {
        Self {
            inner: UntypedSourceArc(Arc::new(Mutex::new(source))),
            marker: PhantomData,
        }
    }

    pub(crate) fn lock(&self) -> SourceStreamGuard<'_, S> {
        SourceStreamGuard {
            inner: self.inner.0.lock(),
            marker: PhantomData,
        }
    }
}

impl<S> Clone for SourceArc<S> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            marker: PhantomData,
        }
    }
}

// === SourceStreamGuard ===

pub(crate) struct SourceStreamGuard<'a, S> {
    inner: MutexGuard<'a, dyn SourceStream>,
    marker: PhantomData<S>,
}

impl<S: 'static> SourceStreamGuard<'_, S> {
    pub(crate) fn pinned(&mut self) -> Pin<&mut S> {
        let stream = self
            .inner
            .as_any_mut()
            .downcast_mut::<S>()
            .expect("invalid source type");

        // SAFETY: The data inside the `Mutex` is pinned and we don't move it out here.
        unsafe { Pin::new_unchecked(stream) }
    }
}

// === UntypedSourceArc ===

#[derive(Clone)]
pub(crate) struct UntypedSourceArc(Arc<Mutex<dyn SourceStream>>);

impl futures::Stream for UntypedSourceArc {
    type Item = Envelope;

    fn poll_next(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<Option<Envelope>> {
        let mut guard = self.0.lock();

        // SAFETY: The data inside the `Mutex` is pinned and we don't move it out here.
        unsafe { Pin::new_unchecked(&mut *guard) }.poll_recv(cx)
    }
}

pub(crate) type Sources = StreamsUnordered<UntypedSourceArc>;
