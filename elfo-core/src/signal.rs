use std::{
    any::Any,
    io,
    os::raw::c_int,
    pin::Pin,
    task::{self, Poll},
};

use pin_project::pin_project;
use sealed::sealed;
#[cfg(unix)]
use tokio::signal;
#[cfg(unix)]
use tokio::signal::unix;
#[cfg(windows)]
use tokio::signal::windows;

use crate::{
    envelope::{Envelope, MessageKind},
    message::Message,
    source::{SourceArc, SourceStream, UnattachedSource},
    tracing::TraceId,
    Addr,
};

/// A source that emits a message once a signal is received.
/// Clones the message on every tick.
///
/// It's based on the tokio implementation, so it should be useful to read
/// about [caveats](https://docs.rs/tokio/latest/tokio/signal/unix/struct.Signal.html).
///
/// # Tracing
///
/// Every message starts a new trace, thus a new trace id is generated and
/// assigned to the current scope.
///
/// # Example
///
/// ```
/// # use std::time::Duration;
/// # use elfo_core as elfo;
/// # async fn exec(mut ctx: elfo::Context) {
/// # use elfo::{message, msg};
/// use elfo::signal::{Signal, SignalKind};
///
/// #[message]
/// struct ReloadFile;
///
/// ctx.attach(Signal::new(SignalKind::UnixHangup, ReloadFile));
///
/// while let Some(envelope) = ctx.recv().await {
///     msg!(match envelope {
///         ReloadFile => { /* ... */ },
///     });
/// }
/// # }
/// ```
pub struct Signal<M> {
    source: SourceArc<SignalSource<M>>,
}

#[sealed]
impl<M: Message> crate::source::SourceHandle for Signal<M> {
    fn is_terminated(&self) -> bool {
        self.source.lock().is_none()
    }

    fn terminate(self) {
        ward!(self.source.lock()).terminate();
    }
}

#[pin_project]
struct SignalSource<M> {
    message: M,
    inner: SignalInner,
}

enum SignalInner {
    Disabled,
    #[cfg(windows)]
    WindowsCtrlC(windows::CtrlC),
    #[cfg(unix)]
    Unix(unix::Signal),
}

/// A kind of signal to listen to.
///
/// * `Unix*` variants are available only on UNIX systems and produce nothing
/// on other systems.
/// * `Windows*` variants are available only on Windows and produce nothing
/// on other systems.
///
/// It helps to avoid writing `#[cfg(_)]` everywhere around signals.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SignalKind {
    /// The "ctrl-c" notification.
    WindowsCtrlC,

    /// Any valid OS signal.
    UnixRaw(c_int),
    /// SIGALRM
    UnixAlarm,
    /// SIGCHLD
    UnixChild,
    /// SIGHUP
    UnixHangup,
    /// SIGINT
    UnixInterrupt,
    /// SIGIO
    UnixIo,
    /// SIGPIPE
    UnixPipe,
    /// SIGQUIT
    UnixQuit,
    /// SIGTERM
    UnixTerminate,
    /// SIGUSR1
    UnixUser1,
    /// SIGUSR2
    UnixUser2,
    /// SIGWINCH
    UnixWindowChange,
}

impl<M: Message> Signal<M> {
    /// Creates an unattached instance of [`Signal`].
    pub fn new(kind: SignalKind, message: M) -> UnattachedSource<Self> {
        let inner = SignalInner::new(kind).unwrap_or_else(|err| {
            tracing::warn!(kind = ?kind, error = %err, "failed to create a signal handler");
            SignalInner::Disabled
        });

        let source = SourceArc::new(SignalSource { message, inner }, false);
        UnattachedSource::new(source, |source| Self { source })
    }

    /// Replaces a stored message with the provided one.
    pub fn set_message(&self, message: M) {
        let mut guard = ward!(self.source.lock());
        *guard.stream().project().message = message;
    }
}

impl SignalInner {
    #[cfg(unix)]
    fn new(kind: SignalKind) -> io::Result<SignalInner> {
        use signal::unix::SignalKind as U;

        let kind = match kind {
            SignalKind::UnixRaw(signum) => U::from_raw(signum),
            SignalKind::UnixAlarm => U::alarm(),
            SignalKind::UnixChild => U::child(),
            SignalKind::UnixHangup => U::hangup(),
            SignalKind::UnixInterrupt => U::interrupt(),
            SignalKind::UnixIo => U::io(),
            SignalKind::UnixPipe => U::pipe(),
            SignalKind::UnixQuit => U::quit(),
            SignalKind::UnixTerminate => U::terminate(),
            SignalKind::UnixUser1 => U::user_defined1(),
            SignalKind::UnixUser2 => U::user_defined2(),
            SignalKind::UnixWindowChange => U::window_change(),
            _ => return Ok(SignalInner::Disabled),
        };

        unix::signal(kind).map(SignalInner::Unix)
    }

    #[cfg(windows)]
    fn new(kind: SignalKind) -> io::Result<SignalInner> {
        match kind {
            SignalKind::WindowsCtrlC => windows::ctrl_c().map(SignalInner::WindowsCtrlC),
            _ => Ok(SignalInner::Disabled),
        }
    }

    fn poll_recv(&mut self, cx: &mut task::Context<'_>) -> Poll<Option<()>> {
        match self {
            SignalInner::Disabled => Poll::Ready(None),
            #[cfg(windows)]
            SignalInner::WindowsCtrlC(inner) => inner.poll_recv(cx),
            #[cfg(unix)]
            SignalInner::Unix(inner) => inner.poll_recv(cx),
        }
    }
}

impl<M: Message> SourceStream for SignalSource<M> {
    fn as_any_mut(self: Pin<&mut Self>) -> Pin<&mut dyn Any> {
        // SAFETY: we only cast here, it cannot move data.
        unsafe { self.map_unchecked_mut(|s| s) }
    }

    fn poll_recv(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<Option<Envelope>> {
        let this = self.project();

        match this.inner.poll_recv(cx) {
            Poll::Ready(Some(())) => {}
            Poll::Ready(None) => return Poll::Ready(None),
            Poll::Pending => return Poll::Pending,
        }

        let message = this.message.clone();
        let kind = MessageKind::Regular { sender: Addr::NULL };
        let trace_id = TraceId::generate();
        let envelope = Envelope::with_trace_id(message, kind, trace_id).upcast();
        Poll::Ready(Some(envelope))
    }
}
