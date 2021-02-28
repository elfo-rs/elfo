use futures_intrusive::{
    buffer::GrowingHeapBuf,
    channel::{self, GenericChannel},
};
use parking_lot::RawMutex;

use crate::envelope::{Envelope, Message};

pub use errors::{SendError, TryRecvError, TrySendError};

mod errors;

// TODO: make mailboxes bounded by time instead of size.
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

    pub(crate) async fn send<M: Message>(
        &self,
        envelope: Envelope<M>,
    ) -> Result<(), SendError<Envelope<M>>> {
        let fut = self.queue.send(envelope.upcast());
        fut.await
            .map_err(|err| err.0.downcast::<M>().expect("Impossible"))
            .map_err(SendError)
    }

    pub(crate) fn try_send<M: Message>(
        &self,
        envelope: Envelope<M>,
    ) -> Result<(), TrySendError<Envelope<M>>> {
        self.queue
            .try_send(envelope.upcast())
            .map_err(|err| match err {
                channel::TrySendError::Full(envelope) => {
                    TrySendError::Full(envelope.downcast().expect("Impossible"))
                }
                channel::TrySendError::Closed(envelope) => {
                    TrySendError::Closed(envelope.downcast().expect("Impossible"))
                }
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
