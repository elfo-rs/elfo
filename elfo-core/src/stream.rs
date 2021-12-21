use std::{
    future::Future,
    mem,
    pin::Pin,
    task::{self, Poll},
};

use futures::{self, channel::mpsc, sink::SinkExt as _, stream, stream::StreamExt as _};
use parking_lot::Mutex;
use sealed::sealed;

use crate::{
    addr::Addr,
    envelope::{Envelope, MessageKind},
    message::Message,
    trace_id::{self, TraceId},
};

// === Stream ===

/// A wrapper around [`futures::Stream`] implementing `Source` trait.
///
/// Stream items must implement [`Message`].
pub struct Stream<S>(Mutex<StreamState<S>>);

enum StreamState<S> {
    Active(Pin<Box<S>>),
    Closed,
}

impl<S> Stream<S> {
    /// Wraps [`futures::Stream`] into the source.
    pub fn new(stream: S) -> Self {
        Self(Mutex::new(StreamState::Active(Box::pin(stream))))
    }

    /// Drops the inner stream and uses the provided one instead.
    pub fn set(&self, stream: S) {
        *self.0.lock() = StreamState::Active(Box::pin(stream));
    }

    /// Replaces the inner stream with the provided one.
    pub fn replace(&self, stream: S) -> Option<S>
    where
        S: Unpin,
    {
        let new_state = StreamState::Active(Box::pin(stream));
        match mem::replace(&mut *self.0.lock(), new_state) {
            StreamState::Active(stream) => Some(*Pin::into_inner(stream)),
            StreamState::Closed => None,
        }
    }

    /// Drops the inner stream and stops emitting messages.
    ///
    /// [`Stream::set`] and [`Stream::replace`] can be used after this method.
    pub fn close(&self) -> bool {
        !matches!(
            mem::replace(&mut *self.0.lock(), StreamState::Closed),
            StreamState::Closed
        )
    }
}

impl Stream<()> {
    /// Generates a stream from the provided generator.
    ///
    /// The generator receives [`Yielder`] as an argument and should return a
    /// future that will produce messages by using [`Yielder::emit`].
    ///
    /// # Examples
    ///
    /// ```ignore
    /// #[message]
    /// struct SomeMessage(u32);
    ///
    /// #[message]
    /// struct AnotherMessage;
    ///
    /// let stream = Stream::generate(|mut y| async move {
    ///     y.emit(SomeMessage(42)).await;
    ///     y.emit(AnotherMessage).await;
    /// });
    ///
    /// let mut ctx = ctx.with(&stream);
    /// ```
    pub fn generate<G, F>(generator: G) -> Stream<impl futures::Stream<Item = Envelope>>
    where
        G: FnOnce(Yielder) -> F,
        F: Future<Output = ()>,
    {
        // Highly inspired by https://github.com/Riateche/stream_generator.
        let (tx, rx) = mpsc::channel(0);
        let gen = generator(Yielder(tx));
        let fake = stream::once(gen).filter_map(|_| async { None });
        Stream::new(stream::select(fake, rx))
    }
}

#[sealed]
impl<S> crate::source::Source for Stream<S>
where
    S: futures::Stream,
    S::Item: StreamItem,
{
    fn poll_recv(&self, cx: &mut task::Context<'_>) -> Poll<Option<Envelope>> {
        let mut state = self.0.lock();

        let stream = match &mut *state {
            StreamState::Active(stream) => stream,
            StreamState::Closed => return Poll::Pending, // TODO: `Poll::Ready(None)`?
        };

        // TODO: should we poll streams in a separate scope?
        match stream.as_mut().poll_next(cx) {
            Poll::Ready(Some(item)) => Poll::Ready(Some(item.unify())),
            Poll::Ready(None) => {
                *state = StreamState::Closed;
                Poll::Ready(None)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

// === Yielder ===

/// A handle for emitting messages from [`Stream::generate`].
pub struct Yielder(mpsc::Sender<Envelope>);

impl Yielder {
    /// Emits a message from the generated stream.
    pub async fn emit<M: Message>(&mut self, message: M) {
        let _ = self.0.send(message.unify()).await;
    }
}

// === StreamItem ===

#[doc(hidden)]
#[sealed]
pub trait StreamItem {
    #[doc(hidden)]
    fn unify(self) -> Envelope;
}

// TODO(v0.2): it's inconsistent with `ctx.send()` that accepts only messages.
#[doc(hidden)]
#[sealed]
impl StreamItem for Envelope {
    #[doc(hidden)]
    fn unify(self) -> Envelope {
        self
    }
}

// TODO(v0.2): remove it, use explicit `scope::set_trace_id()` instead.
#[doc(hidden)]
#[sealed]
impl<M: Message> StreamItem for (TraceId, M) {
    #[doc(hidden)]
    fn unify(self) -> Envelope {
        let kind = MessageKind::Regular { sender: Addr::NULL };
        Envelope::with_trace_id(self.1, kind, self.0).upcast()
    }
}

#[doc(hidden)]
#[sealed]
impl<M: Message> StreamItem for M {
    #[doc(hidden)]
    fn unify(self) -> Envelope {
        (trace_id::generate(), self).unify()
    }
}
