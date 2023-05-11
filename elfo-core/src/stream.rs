use std::{
    any::Any,
    future::Future,
    pin::Pin,
    task::{self, Poll},
};

use futures::{self, channel::mpsc, sink::SinkExt as _, stream, stream::StreamExt as _};
use pin_project::pin_project;
use sealed::sealed;

use crate::{
    addr::Addr,
    envelope::{Envelope, MessageKind},
    message::{AnyMessage, Message},
    scope::{self, Scope},
    source::{SourceArc, SourceStream, UnattachedSource, UntypedSourceArc},
    tracing::TraceId,
};

// === Stream ===

/// A source that emits messages from a provided stream or future.
///
/// Possible items of a stream (the `M` parameter):
/// * Any instance of [`Message`].
/// * `Result<impl Message, impl Message>`.
///
/// Note: the `new()` constructor is reserved until `AsyncIterator` is
/// [stabilized](https://github.com/rust-lang/rust/issues/79024).
///
/// All wrapped streams and futures are fused by the implementation.
///
/// # Tracing
///
/// * If the stream created using [`Stream::from_futures03()`], every message
///   starts a new trace.
/// * If created using [`Stream::once()`], the current trace is preserved.
/// * If created using [`Stream::generate()`], the current trace is preserved.
///
/// You can always use [`scope::set_trace_id()`] to override the current trace.
///
/// # Examples
///
/// Create a stream based on [`futures::Stream`]:
/// ```
/// # use elfo_core as elfo;
/// # async fn exec(mut ctx: elfo::Context) {
/// # use elfo::{message, msg};
/// use elfo::stream::Stream;
///
/// #[message]
/// struct MyItem(u32);
///
/// let stream = futures::stream::iter(vec![MyItem(0), MyItem(1)]);
/// ctx.attach(Stream::from_futures03(stream));
///
/// while let Some(envelope) = ctx.recv().await {
///     msg!(match envelope {
///         MyItem => { /* ... */ },
///     });
/// }
/// # }
/// ```
///
/// Perform a background request:
/// ```
/// # use elfo_core as elfo;
/// # async fn exec(mut ctx: elfo::Context) {
/// # use elfo::{message, msg};
/// # #[message]
/// # struct SomeEvent;
/// use elfo::stream::Stream;
///
/// #[message]
/// struct DataFetched(u32);
///
/// #[message]
/// struct FetchDataFailed(String);
///
/// async fn fetch_data() -> Result<DataFetched, FetchDataFailed> {
///     // ...
/// # todo!()
/// }
///
/// while let Some(envelope) = ctx.recv().await {
///     msg!(match envelope {
///         SomeEvent => {
///             ctx.attach(Stream::once(fetch_data()));
///         },
///         DataFetched => { /* ... */ },
///         FetchDataFailed => { /* ... */ },
///     });
/// }
/// # }
/// ```
///
/// Generate a stream (an alternative to `async-stream`):
/// ```
/// # use elfo_core as elfo;
/// # async fn exec(mut ctx: elfo::Context) {
/// # use elfo::{message, msg};
/// use elfo::stream::Stream;
///
/// #[message]
/// struct SomeMessage(u32);
///
/// #[message]
/// struct AnotherMessage;
///
/// ctx.attach(Stream::generate(|mut e| async move {
///     e.emit(SomeMessage(42)).await;
///     e.emit(AnotherMessage).await;
/// }));
///
/// while let Some(envelope) = ctx.recv().await {
///     msg!(match envelope {
///         SomeMessage(no) | AnotherMessage => { /* ... */ },
///     });
/// }
/// # }
/// ```
pub struct Stream<M = AnyMessage> {
    source: SourceArc<StreamSource<dyn futures::Stream<Item = M> + Send + 'static>>,
}

#[sealed]
impl<M: StreamItem> crate::source::SourceHandle for Stream<M> {
    fn is_terminated(&self) -> bool {
        self.source.is_terminated()
    }

    fn terminate(self) {
        self.source.terminate();
    }
}

impl<M: StreamItem> Stream<M> {
    /// Creates an unattached source based on the provided [`futures::Stream`].
    pub fn from_futures03<S>(stream: S) -> UnattachedSource<Self>
    where
        S: futures::Stream<Item = M> + Send + 'static,
    {
        Self::from_futures03_inner(stream, true)
    }

    /// Creates an uattached source based on the provided future.
    pub fn once<F>(future: F) -> UnattachedSource<Self>
    where
        F: Future<Output = M> + Send + 'static,
    {
        Self::from_futures03_inner(stream::once(future), false)
    }

    fn from_futures03_inner(
        stream: impl futures::Stream<Item = M> + Send + 'static,
        rewrite_trace_id: bool,
    ) -> UnattachedSource<Self> {
        let source = StreamSource {
            scope: scope::expose(),
            rewrite_trace_id,
            inner: stream,
        };

        if rewrite_trace_id {
            source.scope.set_trace_id(TraceId::generate());
        }

        let this = Self {
            // See comments for `from_untyped` to get details why we use it directly here.
            source: SourceArc::from_untyped(UntypedSourceArc::new(source)),
        };

        UnattachedSource::new(this.source.clone(), this)
    }
}

impl Stream<AnyMessage> {
    /// Generates a stream from the provided generator.
    ///
    /// The generator receives [`Emitter`] as an argument and should return a
    /// future that will produce messages by using [`Emitter::emit`].
    pub fn generate<G, F>(generator: G) -> UnattachedSource<Self>
    where
        G: FnOnce(Emitter) -> F,
        F: Future<Output = ()> + Send + 'static,
    {
        // Highly inspired by https://github.com/Riateche/stream_generator.
        // TODO: `mpsc::channel` produces overhead here, replace with a custom slot.
        let (tx, rx) = mpsc::channel(0);
        let gen = generator(Emitter(tx));
        let gen = stream::once(gen).filter_map(|_| async { None });
        let stream = stream::select(gen, rx);

        Self::from_futures03_inner(stream, false)
    }
}

#[pin_project]
struct StreamSource<S: ?Sized> {
    scope: Scope,
    rewrite_trace_id: bool,
    #[pin]
    inner: S,
}

impl<S, M> SourceStream for StreamSource<S>
where
    S: futures::Stream<Item = M> + ?Sized + Send + 'static,
    M: StreamItem,
{
    fn as_any_mut(&mut self) -> &mut dyn Any {
        // We never call `SourceArc::lock().pinned()`, so it can be unimplemented.
        // Anyway, it cannot be implemented because `StreamSource<_>` is DST.
        unreachable!()
    }

    fn poll_recv(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<Option<Envelope>> {
        let this = self.project();

        // TODO: get rid of cloning here.
        // tokio's `LocalKey` API forces us to clone the current scope on every poll.
        // Usually, it's not a problem, but `Scope` contains a shared `Arc` among
        // all actors inside a group, which can lead to a high contention.
        // We can avoid it by implementing a custom `LocalKey`.
        let scope = this.scope.clone();

        scope.sync_within(|| match this.inner.poll_next(cx) {
            Poll::Ready(Some(msg)) => {
                let trace_id = scope::trace_id();

                this.scope.set_trace_id(if *this.rewrite_trace_id {
                    TraceId::generate()
                } else {
                    trace_id
                });

                let msg = msg.to_any_message();
                let kind = MessageKind::Regular { sender: Addr::NULL };
                let envelope = Envelope::with_trace_id(msg, kind, trace_id);

                Poll::Ready(Some(envelope))
            }
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => {
                this.scope.set_trace_id(scope::trace_id());
                Poll::Pending
            }
        })
    }
}

// === Emitter ===

/// A handle for emitting messages from [`Stream::generate`].
pub struct Emitter(mpsc::Sender<AnyMessage>);

impl Emitter {
    /// Emits a message from the generated stream.
    pub async fn emit<M: Message>(&mut self, message: M) {
        let _ = self.0.send(AnyMessage::new(message)).await;
    }
}

// === StreamItem ===

#[sealed]
pub trait StreamItem: 'static {
    /// This method is private.
    #[doc(hidden)]
    fn to_any_message(self) -> AnyMessage;
}

#[sealed]
impl StreamItem for AnyMessage {
    /// This method is private.
    #[doc(hidden)]
    fn to_any_message(self) -> AnyMessage {
        self
    }
}

#[sealed]
impl<M: Message> StreamItem for M {
    /// This method is private.
    #[doc(hidden)]
    fn to_any_message(self) -> AnyMessage {
        AnyMessage::new(self)
    }
}

#[sealed]
impl<M1: Message, M2: Message> StreamItem for Result<M1, M2> {
    /// This method is private.
    #[doc(hidden)]
    fn to_any_message(self) -> AnyMessage {
        match self {
            Ok(msg) => msg.to_any_message(),
            Err(msg) => msg.to_any_message(),
        }
    }
}
