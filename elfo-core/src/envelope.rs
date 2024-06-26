use std::{alloc, fmt, mem, ptr, ptr::NonNull};

use elfo_utils::time::Instant;

use crate::{
    mailbox,
    message::{AnyMessageRef, Message, MessageRepr, MessageTypeId, Request},
    request_table::{RequestId, ResponseToken},
    tracing::TraceId,
    Addr,
};

/// An envelope is a wrapper around message with additional metadata,
/// involved in message passing between actors.
///
/// Envelopes aren't created directly in code, but are produced internally
/// by [`Context`]'s methods.
///
/// Converting an envelope to a message is usually done by calling the [`msg!`]
/// macro, which supports both owned and borrowed usages.
///
/// [`Context`]: crate::Context
/// [`msg!`]: crate::msg
pub struct Envelope(NonNull<EnvelopeHeader>);

// Messages aren't required to be `Sync`.
assert_not_impl_any!(Envelope: Sync);
assert_impl_all!(Envelope: Send);
assert_eq_size!(Envelope, usize);

// TODO: the current size (on x86-64) is 64 bytes, but it can be reduced.
// And... it should be reduced once `TraceId` is extended to 16 bytes.
pub(crate) struct EnvelopeHeader {
    /// See [`mailbox.rs`] for more details.
    pub(crate) link: mailbox::Link,
    created_time: Instant, // Now used also as a sent time.
    trace_id: TraceId,
    kind: MessageKind,
    /// Offset from the beginning of the envelope to the `MessageRepr`.
    message_offset: u32,
}

assert_impl_all!(EnvelopeHeader: Send);

// SAFETY: `Envelope` can point to `M: Message` only, which is `Send`.
// `EnvelopeHeader` is checked statically above to be `Send`.
unsafe impl Send for Envelope {}

// Reexported in `elfo::_priv`.
pub enum MessageKind {
    Regular { sender: Addr },
    RequestAny(ResponseToken),
    RequestAll(ResponseToken),
    Response { sender: Addr, request_id: RequestId },
}

// Called if the envelope hasn't been unpacked at all.
// For instance, if an actor dies with a non-empty mailbox.
// Usually, an envelope goes to `std::mem:forget()` in `unpack_*` methods.
impl Drop for Envelope {
    fn drop(&mut self) {
        let message = self.message();
        let message_layout = message._repr_layout();
        let (layout, message_offset) = envelope_repr_layout(message_layout);
        debug_assert_eq!(message_offset, self.header().message_offset);

        // Drop the message.
        // SAFETY: the message is not accessed anymore below.
        unsafe { message.drop_in_place() };

        // Drop the header.
        // SAFETY: the header is not accessed anymore below.
        unsafe { ptr::drop_in_place(self.0.as_ptr()) }

        // Deallocate the whole envelope.
        // SAFETY: memory was allocated by `alloc::alloc` with the same layout.
        unsafe { alloc::dealloc(self.0.as_ptr().cast(), layout) };
    }
}

impl Envelope {
    // This is private API. Do not use it.
    #[doc(hidden)]
    #[inline]
    pub fn new<M: Message>(message: M, kind: MessageKind) -> Self {
        Self::with_trace_id(message, kind, crate::scope::trace_id())
    }

    // This is private API. Do not use it.
    #[doc(hidden)]
    #[inline]
    pub fn with_trace_id<M: Message>(message: M, kind: MessageKind, trace_id: TraceId) -> Self {
        let message_layout = message._repr_layout();
        let (layout, message_offset) = envelope_repr_layout(message_layout);

        let header = EnvelopeHeader {
            link: <_>::default(),
            created_time: Instant::now(),
            trace_id,
            kind,
            message_offset,
        };

        // SAFETY: `layout` is correct and non-zero.
        let ptr = unsafe { alloc::alloc(layout) };

        let Some(ptr) = NonNull::new(ptr) else {
            alloc::handle_alloc_error(layout);
        };

        // SAFETY: `ptr` is valid to write the header.
        unsafe { ptr::write(ptr.cast().as_ptr(), header) };

        let this = Self(ptr.cast());
        let message_ptr = this.message_repr_ptr();

        // SAFETY: `message_ptr` is valid to write the message.
        unsafe { message._write(message_ptr) };

        this
    }

    pub(crate) fn stub() -> Self {
        Self::with_trace_id(
            crate::messages::Ping,
            MessageKind::Regular { sender: Addr::NULL },
            TraceId::try_from(1).unwrap(),
        )
    }

    fn header(&self) -> &EnvelopeHeader {
        // SAFETY: `self.0` is properly initialized.
        unsafe { self.0.as_ref() }
    }

    #[inline]
    pub fn trace_id(&self) -> TraceId {
        self.header().trace_id
    }

    /// Returns a reference to the untyped message inside the envelope.
    #[inline]
    pub fn message(&self) -> AnyMessageRef<'_> {
        // TODO: rename to `message_ref`?
        let message_repr = self.message_repr_ptr();

        // SAFETY: `message_repr` is valid pointer for read.
        unsafe { AnyMessageRef::new(message_repr) }
    }

    /// Part of private API. Do not use it.
    #[doc(hidden)]
    pub fn message_kind(&self) -> &MessageKind {
        &self.header().kind
    }

    pub(crate) fn created_time(&self) -> Instant {
        self.header().created_time
    }

    #[inline]
    pub fn sender(&self) -> Addr {
        match self.message_kind() {
            MessageKind::Regular { sender } => *sender,
            MessageKind::RequestAny(token) => token.sender(),
            MessageKind::RequestAll(token) => token.sender(),
            MessageKind::Response { sender, .. } => *sender,
        }
    }

    #[doc(hidden)]
    #[inline]
    pub fn type_id(&self) -> MessageTypeId {
        self.message().type_id()
    }

    #[inline]
    pub fn is<M: Message>(&self) -> bool {
        self.message().is::<M>()
    }

    #[doc(hidden)]
    pub fn duplicate(&self) -> Self {
        let header = self.header();
        let message = self.message();
        let message_layout = message._repr_layout();
        let (layout, message_offset) = envelope_repr_layout(message_layout);
        debug_assert_eq!(message_offset, header.message_offset);

        let out_header = EnvelopeHeader {
            link: <_>::default(),
            created_time: header.created_time,
            trace_id: header.trace_id,
            kind: match &header.kind {
                MessageKind::Regular { sender } => MessageKind::Regular { sender: *sender },
                MessageKind::RequestAny(token) => MessageKind::RequestAny(token.duplicate()),
                MessageKind::RequestAll(token) => MessageKind::RequestAll(token.duplicate()),
                MessageKind::Response { sender, request_id } => MessageKind::Response {
                    sender: *sender,
                    request_id: *request_id,
                },
            },
            message_offset,
        };

        // SAFETY: `layout` is correct and non-zero.
        let out_ptr = unsafe { alloc::alloc(layout) };

        let Some(out_ptr) = NonNull::new(out_ptr) else {
            alloc::handle_alloc_error(layout);
        };

        // SAFETY: `out_ptr` is valid to write the header.
        unsafe { ptr::write(out_ptr.cast().as_ptr(), out_header) };

        let out = Self(out_ptr.cast());
        let out_message_ptr = out.message_repr_ptr();

        // SAFETY: `out_message_ptr` is valid and has the same layout as `message`.
        unsafe { message.clone_into(out_message_ptr) };

        out
    }

    // TODO: remove the method
    pub(crate) fn set_message<M: Message>(&mut self, message: M) {
        assert!(self.is::<M>() && M::_type_id() != crate::message::AnyMessage::_type_id());

        // TODO: rewrite without `MessageRepr`.
        let repr_ptr = self.message_repr_ptr().cast::<MessageRepr<M>>().as_ptr();

        // SAFETY: `repr_ptr` is valid to write the message.
        unsafe { ptr::replace(repr_ptr, MessageRepr::new(message)) };
    }

    fn message_repr_ptr(&self) -> NonNull<MessageRepr> {
        let message_offset = self.header().message_offset;

        // SAFETY: `message_offset` refers to the same allocation object.
        let ptr = unsafe { self.0.as_ptr().cast::<u8>().add(message_offset as usize) };

        // SAFETY: `envelope_repr_layout()` guarantees that `ptr` is valid.
        unsafe { NonNull::new_unchecked(ptr.cast()) }
    }

    #[doc(hidden)]
    #[inline]
    pub fn unpack<M: Message>(self) -> Option<(M, MessageKind)> {
        self.is::<M>()
            // SAFETY: `self` contains a message of type `M`, checked above.
            .then(|| unsafe { self.unpack_unchecked() })
    }

    /// # Safety
    ///
    /// The caller must ensure that the message is of the correct type.
    unsafe fn unpack_unchecked<M: Message>(self) -> (M, MessageKind) {
        let message_layout = self.message()._repr_layout();
        let (layout, message_offset) = envelope_repr_layout(message_layout);
        debug_assert_eq!(message_offset, self.header().message_offset);

        let message = M::_read(self.message_repr_ptr());
        let kind = ptr::read(&self.0.as_ref().kind);

        alloc::dealloc(self.0.as_ptr().cast(), layout);
        mem::forget(self);
        (message, kind)
    }

    pub(crate) fn drop_as_unused(mut self) {
        // SAFETY: `self` is properly initialized.
        let header = unsafe { self.0.as_mut() };

        if let MessageKind::RequestAny(token) | MessageKind::RequestAll(token) = &mut header.kind {
            // FIXME: probably invalid for ALL requests, need to decrement remainder.
            // REVIEW: DO NOT forget check & fix it before merging.
            token.forget();
        }
    }

    pub(crate) fn into_header_ptr(self) -> NonNull<EnvelopeHeader> {
        let ptr = self.0;
        mem::forget(self);
        ptr
    }

    pub(crate) unsafe fn from_header_ptr(ptr: NonNull<EnvelopeHeader>) -> Self {
        Self(ptr)
    }
}

fn envelope_repr_layout(message_layout: alloc::Layout) -> (alloc::Layout, u32) {
    let (layout, message_offset) = alloc::Layout::new::<EnvelopeHeader>()
        .extend(message_layout)
        .expect("impossible envelope layout");

    let message_offset =
        u32::try_from(message_offset).expect("message requires too large alignment");

    (layout.pad_to_align(), message_offset)
}

impl fmt::Debug for MessageKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MessageKind::Regular { sender: _ } => f.debug_struct("Regular").finish(),
            MessageKind::RequestAny(token) => f
                .debug_tuple("RequestAny")
                .field(&token.request_id())
                .finish(),
            MessageKind::RequestAll(token) => f
                .debug_tuple("RequestAll")
                .field(&token.request_id())
                .finish(),
            MessageKind::Response {
                sender: _,
                request_id,
            } => f.debug_tuple("Response").field(request_id).finish(),
        }
    }
}

impl fmt::Debug for Envelope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Envelope")
            .field("trace_id", &self.trace_id())
            .field("sender", &self.sender())
            .field("kind", &self.message_kind())
            .field("message", &self.message())
            .finish()
    }
}

// Extra traits to support both owned and borrowed usages of `msg!(..)`.

pub trait EnvelopeOwned {
    /// # Safety
    ///
    /// The caller must ensure that the message is of the correct type.
    unsafe fn unpack_regular_unchecked<M: Message>(self) -> M;

    /// # Safety
    ///
    /// The caller must ensure that the request is of the correct type.
    unsafe fn unpack_request_unchecked<R: Request>(self) -> (R, ResponseToken<R>);
}

pub trait EnvelopeBorrowed {
    /// # Safety
    ///
    /// The caller must ensure that the message is of the correct type.
    unsafe fn unpack_regular_unchecked<M: Message>(&self) -> &M;
}

// TODO: AnyMessage
impl EnvelopeOwned for Envelope {
    #[inline]
    unsafe fn unpack_regular_unchecked<M: Message>(self) -> M {
        let (message, kind) = self.unpack_unchecked();

        #[cfg(feature = "network")]
        if let MessageKind::RequestAny(token) | MessageKind::RequestAll(token) = kind {
            // The sender thought this is a request, but for the current node it isn't.
            // Mark the token as received to return `RequestError::Ignored` to the sender.
            let _ = token.into_received::<()>();
        }

        // TODO: about assert in `msg!`
        #[cfg(not(feature = "network"))]
        debug_assert!(!matches!(
            kind,
            MessageKind::RequestAny(_) | MessageKind::RequestAll(_)
        ));

        message
    }

    #[inline]
    unsafe fn unpack_request_unchecked<R: Request>(self) -> (R, ResponseToken<R>) {
        let (message, kind) = self.unpack_unchecked();

        let token = match kind {
            MessageKind::RequestAny(token) | MessageKind::RequestAll(token) => token,
            // A request sent by using `ctx.send()` ("fire and forget").
            // Also it's useful for the protocol evolution between remote nodes.
            _ => ResponseToken::forgotten(),
        };

        (message, token.into_received())
    }
}

impl EnvelopeBorrowed for Envelope {
    #[inline]
    unsafe fn unpack_regular_unchecked<M: Message>(&self) -> &M {
        self.message().downcast_ref_unchecked()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use elfo_utils::time;

    use super::*;
    use crate::message;

    #[message]
    #[derive(PartialEq)]
    struct Sample {
        value: u128,
        counter: Arc<()>,
    }

    impl Sample {
        fn new(value: u128) -> (Arc<()>, Self) {
            let this = Self {
                value,
                counter: Arc::new(()),
            };

            (this.counter.clone(), this)
        }
    }

    #[test]
    fn duplicate_miri() {
        let (counter, message) = Sample::new(42);

        // Miri doesn't support asm, so mock the time.
        let envelope = time::with_instant_mock(|_mock| {
            Envelope::with_trace_id(
                message,
                MessageKind::Regular { sender: Addr::NULL },
                TraceId::try_from(1).unwrap(),
            )
        });

        assert_eq!(Arc::strong_count(&counter), 2);
        let envelope2 = envelope.duplicate();
        assert_eq!(Arc::strong_count(&counter), 3);
        assert!(envelope2.is::<Sample>());
        let envelope3 = envelope2.duplicate();
        assert_eq!(Arc::strong_count(&counter), 4);
        assert!(envelope3.is::<Sample>());

        drop(envelope2);
        assert_eq!(Arc::strong_count(&counter), 3);

        drop(envelope3);
        assert_eq!(Arc::strong_count(&counter), 2);

        let envelope4 = envelope.duplicate();
        assert_eq!(Arc::strong_count(&counter), 3);
        assert!(envelope4.is::<Sample>());

        drop(envelope);
        assert_eq!(Arc::strong_count(&counter), 2);

        drop(envelope4);
        assert_eq!(Arc::strong_count(&counter), 1);
    }
}
