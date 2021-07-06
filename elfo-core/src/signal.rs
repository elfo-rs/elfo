use std::{
    io,
    os::raw::c_int,
    task::{self, Poll},
};

use parking_lot::Mutex;
#[cfg(unix)]
use tokio::signal::{self, unix};
use tokio_util::sync::ReusableBoxFuture;

use crate::{
    addr::Addr,
    context::Source,
    envelope::{Envelope, MessageKind},
    message::Message,
    trace_id,
};

/// `Source` that watches signals.
///
/// All signals except `SignalKind::CtrlC` produces messages on UNIX system
/// only. For other systems they produce nothing. It's useful and helps to
/// avoid writing `#[cfg(unix)]` everywhere around signals.
pub struct Signal<F> {
    inner: Mutex<SignalInner>,
    message_factory: F,
}

enum SignalInner {
    CtrlC(ReusableBoxFuture<io::Result<()>>),
    #[cfg(unix)]
    Unix(unix::Signal),
    Empty,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SignalKind {
    /// Completes when a “ctrl-c” notification is sent to the process.
    ///
    /// See https://docs.rs/tokio/1.7.1/tokio/signal/fn.ctrl_c.html
    CtrlC,

    /// Allows for listening to any valid OS signal.
    Raw(c_int),
    /// SIGALRM
    Alarm,
    /// SIGCHLD
    Child,
    /// SIGHUP
    Hangup,
    /// SIGINT
    Interrupt,
    /// SIGIO
    Io,
    /// SIGPIPE
    Pipe,
    /// SIGQUIT
    Quit,
    /// SIGTERM
    Terminate,
    /// SIGUSR1
    User1,
    /// SIGUSR2
    User2,
    /// SIGWINCH
    WindowChange,
}

impl<F> Signal<F> {
    pub fn new(kind: SignalKind, message_factory: F) -> Self {
        let inner = match kind {
            // TODO: remove this line for `unix` after testing windows.
            SignalKind::CtrlC => SignalInner::CtrlC(ReusableBoxFuture::new(signal::ctrl_c())),
            #[cfg(unix)]
            _ => match create_by_kind(kind) {
                Ok(inner) => SignalInner::Unix(inner),
                Err(err) => {
                    tracing::warn!(
                        kind = ?kind,
                        error = %err,
                        "failed to create a signal handler"
                    );
                    SignalInner::Empty
                }
            },
            #[cfg(not(unix))]
            _ => SignalInner::Empty,
        };

        Self {
            inner: Mutex::new(inner),
            message_factory,
        }
    }
}

#[cfg(unix)]
fn create_by_kind(kind: SignalKind) -> io::Result<unix::Signal> {
    use signal::unix::SignalKind as TSK;

    let kind = match kind {
        SignalKind::CtrlC => TSK::interrupt(),
        SignalKind::Raw(signum) => TSK::from_raw(signum),
        SignalKind::Alarm => TSK::alarm(),
        SignalKind::Child => TSK::child(),
        SignalKind::Hangup => TSK::hangup(),
        SignalKind::Interrupt => TSK::interrupt(),
        SignalKind::Io => TSK::io(),
        SignalKind::Pipe => TSK::pipe(),
        SignalKind::Quit => TSK::quit(),
        SignalKind::Terminate => TSK::terminate(),
        SignalKind::User1 => TSK::user_defined1(),
        SignalKind::User2 => TSK::user_defined2(),
        SignalKind::WindowChange => TSK::window_change(),
    };

    unix::signal(kind)
}

impl<M, F> Source for Signal<F>
where
    F: Fn() -> M,
    M: Message,
{
    fn poll_recv(&self, cx: &mut task::Context<'_>) -> Poll<Option<Envelope>> {
        match &mut *self.inner.lock() {
            SignalInner::CtrlC(inner) => {
                if let Err(err) = futures::ready!(inner.poll(cx)) {
                    tracing::error!(error = %err, "failed to receive a signal");
                }

                assert!(inner.try_set(signal::ctrl_c()).is_ok());
            }
            SignalInner::Unix(inner) => {
                if !matches!(inner.poll_recv(cx), Poll::Ready(Some(()))) {
                    return Poll::Pending;
                }
            }
            SignalInner::Empty => return Poll::Pending,
        };

        let message = (self.message_factory)();
        let kind = MessageKind::Regular { sender: Addr::NULL };
        let trace_id = trace_id::generate();
        let envelope = Envelope::with_trace_id(message, kind, trace_id).upcast();
        Poll::Ready(Some(envelope))
    }
}

#[cfg(test)]
#[cfg(feature = "test-util")]
mod tests {
    use super::*;

    use futures::{future::poll_fn, poll};

    use elfo_macros::message;

    use crate::assert_msg;

    #[message(elfo = crate)]
    struct SomeSignal;

    #[tokio::test]
    #[cfg(unix)]
    async fn unix_signal() {
        let signal = Signal::new(SignalKind::User1, || SomeSignal);

        for _ in 0..=5 {
            assert!(poll!(poll_fn(|cx| signal.poll_recv(cx))).is_pending());
            send_signal(libc::SIGUSR1);
            assert_msg!(
                poll_fn(|cx| signal.poll_recv(cx)).await.unwrap(),
                SomeSignal
            );
        }
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn ctrl_c() {
        let signal = Signal::new(SignalKind::CtrlC, || SomeSignal);

        for _ in 0..=5 {
            assert!(poll!(poll_fn(|cx| signal.poll_recv(cx))).is_pending());
            send_signal(libc::SIGINT);
            assert_msg!(
                poll_fn(|cx| signal.poll_recv(cx)).await.unwrap(),
                SomeSignal
            );
        }
    }

    fn send_signal(signal: libc::c_int) {
        use libc::{getpid, kill};

        unsafe {
            assert_eq!(kill(getpid(), signal), 0);
        }
    }
}
