use futures_intrusive::{
    buffer::GrowingHeapBuf,
    channel::{self, GenericChannel},
};
use parking_lot::{Mutex, RawMutex};

use crate::{
    envelope::Envelope,
    errors::{SendError, TrySendError},
    tracing::TraceId,
};

// TODO: make mailboxes bounded by time instead of size.
const LIMIT: usize = 100_000;

pub(crate) struct Mailbox {
    queue: GenericChannel<RawMutex, Envelope, GrowingHeapBuf<Envelope>>,
    close: Mutex<Option<TraceId>>,
}

impl Mailbox {
    pub(crate) fn new() -> Self {
        Self {
            queue: GenericChannel::with_capacity(LIMIT),
            close: Mutex::new(None),
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

    pub(crate) async fn recv(&self) -> RecvResult {
        let fut = self.queue.receive();
        match fut.await {
            Some(envelope) => RecvResult::Data(envelope),
            None => self.on_close(),
        }
    }

    pub(crate) fn try_recv(&self) -> Option<RecvResult> {
        match self.queue.try_receive() {
            Ok(envelope) => Some(RecvResult::Data(envelope)),
            Err(channel::TryReceiveError::Empty) => None,
            Err(channel::TryReceiveError::Closed) => Some(self.on_close()),
        }
    }

    #[cold]
    pub(crate) fn close(&self, trace_id: TraceId) -> bool {
        if self.queue.close().is_newly_closed() {
            *self.close.lock() = Some(trace_id);
            true
        } else {
            false
        }
    }

    #[cold]
    pub(crate) fn drop_all(&self) {
        while self.queue.try_receive().is_ok() {}
    }

    #[cold]
    fn on_close(&self) -> RecvResult {
        let trace_id = self.close.lock().expect("called before close()");
        RecvResult::Closed(trace_id)
    }
}

#[allow(clippy::large_enum_variant)]
pub(crate) enum RecvResult {
    Data(Envelope),
    Closed(TraceId),
}
