use futures_intrusive::{
    buffer::GrowingHeapBuf,
    channel::{self, GenericChannel},
};
use parking_lot::RawMutex;

use crate::envelope::Envelope;

#[allow(unreachable_pub)]
pub use errors::{SendError, TryRecvError, TrySendError};

mod errors;

// TODO: make mailboxes bounded by time instead of size.
// TODO: use it.
const LIMIT: usize = 128;

pub(crate) struct Mailbox {
    queue: GenericChannel<RawMutex, Envelope, GrowingHeapBuf<Envelope>>,
}

impl Mailbox {
    pub(crate) fn new() -> Self {
        Self {
            // TODO: restrict the size.
            queue: GenericChannel::new(),
        }
    }

    pub(crate) async fn send(&self, envelope: Envelope) -> Result<(), SendError<Envelope>> {
        let fut = self.queue.send(envelope);
        fut.await.map_err(|err| SendError(err.0))
    }

    pub(crate) fn try_send(&self, envelope: Envelope) -> Result<(), TrySendError<Envelope>> {
        self.queue.try_send(envelope).map_err(|err| match err {
            channel::TrySendError::Full(envelope) => TrySendError::Full(envelope),
            channel::TrySendError::Closed(envelope) => TrySendError::Closed(envelope),
        })
    }

    pub(crate) async fn recv(&self) -> Option<Envelope> {
        let fut = self.queue.receive();
        fut.await
    }

    pub(crate) fn try_recv(&self) -> Result<Envelope, TryRecvError> {
        self.queue.try_receive().map_err(|err| match err {
            channel::TryReceiveError::Empty => TryRecvError::Empty,
            channel::TryReceiveError::Closed => TryRecvError::Closed,
        })
    }
}
