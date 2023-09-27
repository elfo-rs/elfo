use crate::{
    addr::Addr,
    envelope::Envelope,
    errors::{RequestError, SendError, TrySendError},
    request_table::ResponseToken,
};

#[stability::unstable]
pub trait RemoteHandle: Send + Sync + 'static {
    fn send(&self, recipient: Addr, envelope: Envelope) -> SendResult;
    fn try_send(&self, recipient: Addr, envelope: Envelope) -> Result<(), TrySendError<Envelope>>;
    fn respond(&self, token: ResponseToken, response: Result<Envelope, RequestError>);
}

#[stability::unstable]
pub enum SendResult {
    Ok,
    Err(SendError<Envelope>),
    Wait(SendNotified, Envelope),
}

pub use self::notifier::*;
mod notifier {
    use std::{
        future::Future,
        pin::Pin,
        sync::atomic::{AtomicUsize, Ordering},
        task,
    };

    use futures_intrusive::sync::{SharedSemaphore, SharedSemaphoreAcquireFuture};
    use pin_project::pin_project;

    #[stability::unstable]
    pub struct SendNotify {
        semaphore: SharedSemaphore,
        waiters: AtomicUsize,
    }

    impl Default for SendNotify {
        fn default() -> Self {
            Self {
                semaphore: SharedSemaphore::new(true, 0),
                waiters: AtomicUsize::new(0),
            }
        }
    }

    impl SendNotify {
        #[stability::unstable]
        pub fn notified(&self) -> SendNotified {
            self.waiters.fetch_add(1, Ordering::SeqCst);
            SendNotified(self.semaphore.acquire(1))
        }

        #[stability::unstable]
        pub fn notify(&self) {
            let waiters = self.waiters.swap(0, Ordering::SeqCst);
            self.semaphore.release(waiters);
        }
    }

    #[stability::unstable]
    #[pin_project]
    pub struct SendNotified(#[pin] SharedSemaphoreAcquireFuture);

    impl Future for SendNotified {
        type Output = ();

        fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> task::Poll<Self::Output> {
            let this = self.project();

            this.0.poll(cx).map(|mut r| {
                r.disarm();
            })
        }
    }
}
