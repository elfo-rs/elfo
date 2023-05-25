use std::collections::VecDeque;

use elfo_utils::{likely, unlikely};
use futures_intrusive::{
    buffer::RingBuf,
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
    queue: GenericChannel<RawMutex, (bool, Envelope), PriorityBuf>,
    close: Mutex<Option<TraceId>>,
}

struct PriorityBuf {
    buffer: VecDeque<(bool, Envelope)>,
    priority_prefix_end: usize,
    limit: usize,
}

impl RingBuf for PriorityBuf {
    type Item = (bool, Envelope);

    fn new() -> Self {
        PriorityBuf {
            buffer: VecDeque::new(),
            priority_prefix_end: 0,
            limit: 0,
        }
    }

    fn with_capacity(limit: usize) -> Self {
        PriorityBuf {
            buffer: VecDeque::new(),
            priority_prefix_end: 0,
            limit,
        }
    }

    #[inline]
    fn capacity(&self) -> usize {
        self.limit
    }

    #[inline]
    fn len(&self) -> usize {
        self.buffer.len()
    }

    #[inline]
    fn can_push(&self) -> bool {
        self.buffer.len() != self.limit
    }

    #[inline]
    fn push(&mut self, value: Self::Item) {
        debug_assert!(self.can_push());
        if unlikely(value.0) {
            // To maintain FIFO order of priority messages, we need to push such messages to
            // the end of the priority prefix in the buffer.
            //
            // |-------------------- Buffer ------------------|
            // |-- Priority messages --|-- Regular messages --|
            //                          ^
            //                          |
            // priority_prefix_end -----+
            if likely(self.priority_prefix_end == 0) {
                self.buffer.push_front(value);
            } else {
                self.buffer.insert(self.priority_prefix_end, value);
            }
            self.priority_prefix_end += 1;
        } else {
            self.buffer.push_back(value);
        }
    }

    #[inline]
    fn pop(&mut self) -> Self::Item {
        debug_assert!(!self.buffer.is_empty());
        let item = self.buffer.pop_front().unwrap();
        debug_assert!(!item.0 || self.priority_prefix_end > 0);
        if unlikely(item.0) {
            self.priority_prefix_end -= 1;
        }
        item
    }
}

impl Mailbox {
    pub(crate) fn new() -> Self {
        Self {
            queue: GenericChannel::with_capacity(LIMIT),
            close: Mutex::new(None),
        }
    }

    pub(crate) async fn send(&self, envelope: Envelope) -> Result<(), SendError<Envelope>> {
        let fut = self.queue.send((false, envelope));
        fut.await.map_err(|err| SendError(err.0 .1))
    }

    pub(crate) async fn send_high_priority(
        &self,
        envelope: Envelope,
    ) -> Result<(), SendError<Envelope>> {
        let fut = self.queue.send((true, envelope));
        fut.await.map_err(|err| SendError(err.0 .1))
    }

    pub(crate) fn try_send(&self, envelope: Envelope) -> Result<(), TrySendError<Envelope>> {
        self.queue
            .try_send((false, envelope))
            .map_err(|err| match err {
                channel::TrySendError::Full(envelope) => TrySendError::Full(envelope.1),
                channel::TrySendError::Closed(envelope) => TrySendError::Closed(envelope.1),
            })
    }

    pub(crate) fn try_send_high_priority(
        &self,
        envelope: Envelope,
    ) -> Result<(), TrySendError<Envelope>> {
        self.queue
            .try_send((true, envelope))
            .map_err(|err| match err {
                channel::TrySendError::Full(envelope) => TrySendError::Full(envelope.1),
                channel::TrySendError::Closed(envelope) => TrySendError::Closed(envelope.1),
            })
    }

    pub(crate) async fn recv(&self) -> RecvResult {
        let fut = self.queue.receive();
        match fut.await {
            Some(envelope) => RecvResult::Data(envelope.1),
            None => self.on_close(),
        }
    }

    pub(crate) fn try_recv(&self) -> Option<RecvResult> {
        match self.queue.try_receive() {
            Ok(envelope) => Some(RecvResult::Data(envelope.1)),
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
