use std::{
    alloc, fmt,
    marker::PhantomData,
    mem::{self, ManuallyDrop},
    ops::Deref,
    ptr::{self, NonNull},
};

use serde::{
    de,
    ser::{self, SerializeStruct as _, SerializeTuple as _},
    Deserialize, Serialize,
};

use super::{Message, MessageRepr, MessageTypeId, MessageVTable};
use crate::{dumping, scope::SerdeMode};

// === AnyMessage ===

/// Heap-allocated message that can be downcasted to a concrete message type.
///
/// It can be thought as a working version of `Box<dyn Message>`.
///
/// [`AnyMessage`] implements [`Message`], thus it can be used in any context
/// where a message is expected, e.g. sending in [`Context`].
///
/// [`AnyMessage`] doesn't implement [`Request`], so it's useful only
/// for regular messages. However, requests are also messages and can
/// be converted to [`AnyMessage`], but usage is limited in this case.
///
/// [`Context`]: crate::Context
/// [`Request`]: crate::Request
pub struct AnyMessage(NonNull<MessageRepr>);

// Messages aren't required to be `Sync`.
assert_not_impl_any!(AnyMessage: Sync);

// SAFETY: `AnyMessage` can point to `M: Message` only, which is `Send`.
unsafe impl Send for AnyMessage {}

impl AnyMessage {
    /// Converts a message into [`AnyMessage`].
    #[inline]
    pub fn new<M: Message>(message: M) -> Self {
        // If `M != AnyMessage` then `from_real()` is called.
        // Otherwise, the message is returned as is.
        message._into_any()
    }

    pub(super) fn from_real<M: Message>(message: M) -> Self {
        debug_assert_ne!(M::_type_id(), Self::_type_id());

        let ptr = alloc_repr(message._vtable());
        // SAFETY: allocation is done for `M`.
        unsafe { message._write(ptr) };
        Self(ptr)
    }

    /// # Safety
    ///
    /// The caller must ensure that the message is of the correct type.
    pub(super) unsafe fn into_real<M: Message>(self) -> M {
        debug_assert_ne!(M::_type_id(), Self::_type_id());

        let data = M::_read(self.0);
        dealloc_repr(self.0);
        mem::forget(self);
        data
    }

    /// # Safety
    ///
    /// The caller must ensure that the message is of the correct type.
    pub(super) unsafe fn as_real_ref<M: Message>(&self) -> &M {
        debug_assert_ne!(M::_type_id(), Self::_type_id());

        &self.0.cast::<MessageRepr<M>>().as_ref().data
    }

    /// Returns [`AnyMessageRef`] that borrows the message.
    pub fn as_ref(&self) -> AnyMessageRef<'_> {
        // SAFETY: `self` is valid for reads.
        unsafe { AnyMessageRef::new(self.0) }
    }

    pub(crate) fn type_id(&self) -> MessageTypeId {
        MessageTypeId::new(self._vtable())
    }

    /// Checks if the message is of a specific type.
    ///
    /// Note: it returns `true` if `M` is [`AnyMessage`].
    #[inline]
    pub fn is<M: Message>(&self) -> bool {
        // If `M != AnyMessage`, it checks that type ids are equal.
        // Otherwise, it returns `true`.
        M::_is_supertype_of(self.type_id())
    }

    /// Tries to downcast the message to a reference to the concrete type.
    ///
    /// Note: it returns `Some(&self)` if `M` is [`AnyMessage`].
    #[inline]
    pub fn downcast_ref<M: Message>(&self) -> Option<&M> {
        self.is::<M>()
            // SAFETY: `self` is of type `M`, checked above.
            .then(|| unsafe { self.downcast_ref_unchecked() })
    }

    /// # Safety
    ///
    /// The caller must ensure that the message is of the correct type.
    pub(crate) unsafe fn downcast_ref_unchecked<M: Message>(&self) -> &M {
        // If `M != AnyMessage` then `as_real_ref()` is called.
        // Otherwise, the message is returned as is.
        M::_from_any_ref(self)
    }

    /// Tries to downcast the message to a concrete type.
    #[inline]
    pub fn downcast<M: Message>(self) -> Result<M, AnyMessage> {
        if !self.is::<M>() {
            return Err(self);
        }

        // SAFETY: `self` is of type `M`, checked above.
        Ok(unsafe { self.downcast_unchecked() })
    }

    /// # Safety
    ///
    /// The caller must ensure that the message is of the correct type.
    unsafe fn downcast_unchecked<M: Message>(self) -> M {
        // If `M != AnyMessage` then `into_real()` is called.
        // Otherwise, the message is returned as is.
        M::_from_any(self)
    }

    /// # Safety
    ///
    /// `out_ptr` must be valid pointer to `MessageRepr<M>`,
    /// where `M` is the same type that is hold by `self`.
    pub(crate) unsafe fn clone_into(&self, out_ptr: NonNull<MessageRepr>) {
        let vtable = self._vtable();
        (vtable.clone)(self.0, out_ptr);
    }

    /// # Safety
    ///
    /// Data behind `self` cannot be accessed after this call.
    pub(crate) unsafe fn drop_in_place(&self) {
        let vtable = self._vtable();
        (vtable.drop)(self.0);
    }

    fn as_serialize(&self) -> &(impl Serialize + ?Sized) {
        let vtable = self._vtable();

        // SAFETY: the resulting reference is bound to the lifetime of `self`.
        unsafe { (vtable.as_serialize_any)(self.0).as_ref() }
    }
}

impl Drop for AnyMessage {
    fn drop(&mut self) {
        // SAFETY: only a vtable will be accessed below.
        unsafe { self.drop_in_place() };

        // SAFETY: memory allocated by `alloc_repr()`.
        unsafe { dealloc_repr(self.0) };
    }
}

impl Clone for AnyMessage {
    fn clone(&self) -> Self {
        let out_ptr = alloc_repr(self._vtable());

        // SAFETY: `out_ptr` is based on the same message as `self`.
        unsafe { self.clone_into(out_ptr) };

        Self(out_ptr)
    }
}

impl fmt::Debug for AnyMessage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // SAFETY: the vtable belongs to `self`.
        unsafe { (self._vtable().debug)(self.0, f) }
    }
}

fn alloc_repr(vtable: &'static MessageVTable) -> NonNull<MessageRepr> {
    // SAFETY:
    // * `vtable` is valid (can be created only by `MessageVTable::new()`).
    // * `vtable.repr_layout` is non-zero.
    let ptr = unsafe { alloc::alloc(vtable.repr_layout) };

    let Some(ptr) = NonNull::new(ptr) else {
        alloc::handle_alloc_error(vtable.repr_layout);
    };

    ptr.cast()
}

/// # Safety
///
/// `ptr` must denote a block of memory allocated by [`alloc_repr()`].
unsafe fn dealloc_repr(ptr: NonNull<MessageRepr>) {
    let ptr = ptr.as_ptr();
    let vtable = (*ptr).vtable;

    alloc::dealloc(ptr.cast(), vtable.repr_layout);
}

impl Message for AnyMessage {
    #[inline(always)]
    fn _type_id() -> MessageTypeId {
        MessageTypeId::any()
    }

    #[inline(always)]
    fn _vtable(&self) -> &'static MessageVTable {
        // SAFETY: `self` refers to the valid object.
        unsafe { (*self.0.as_ptr()).vtable }
    }

    #[inline(always)]
    fn _is_supertype_of(_: MessageTypeId) -> bool {
        true
    }

    #[inline(always)]
    fn _touch(&self) {}

    #[inline(always)]
    fn _into_any(self) -> AnyMessage {
        self
    }

    #[inline(always)]
    unsafe fn _from_any(any: AnyMessage) -> Self {
        any
    }

    #[inline(always)]
    unsafe fn _from_any_ref(any: &AnyMessage) -> &Self {
        any
    }

    #[inline(always)]
    fn _erase(&self) -> dumping::ErasedMessage {
        let vtable = self._vtable();

        // SAFETY: the vtable belongs to `self`.
        unsafe { (vtable.erase)(self.0) }
    }

    #[inline(always)]
    unsafe fn _read(ptr: NonNull<MessageRepr>) -> Self {
        let vtable = (*ptr.as_ptr()).vtable;
        let this = alloc_repr(vtable);

        ptr::copy_nonoverlapping(
            ptr.cast::<u8>().as_ptr(),
            this.cast::<u8>().as_ptr(),
            vtable.repr_layout.size(),
        );

        Self(this)
    }

    #[inline(always)]
    unsafe fn _write(self, out_ptr: NonNull<MessageRepr>) {
        ptr::copy_nonoverlapping(
            self.0.cast::<u8>().as_ptr(),
            out_ptr.cast::<u8>().as_ptr(),
            self._vtable().repr_layout.size(),
        );

        dealloc_repr(self.0);

        mem::forget(self);
    }
}

// `Serialize` / `Deserialize` impls for `AnyMessage` are not used when sending
// it by itself over network (e.g. using `ctx.send(msg)`) or dumping.
// However, it's used if
// * It's a part of another message (e.g. `struct Msg(AnyMessage)`).
// * It's serialized directly (e.g. `insta::assert_yaml_snapshot!(msg)`).
impl Serialize for AnyMessage {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: ser::Serializer,
    {
        // TODO: use compact form only for network?
        if crate::scope::serde_mode() == SerdeMode::Dumping {
            let mut fields = serializer.serialize_struct("AnyMessage", 3)?;
            fields.serialize_field("protocol", self.protocol())?;
            fields.serialize_field("name", self.name())?;
            fields.serialize_field("payload", self.as_serialize())?;
            fields.end()
        } else {
            let mut tuple = serializer.serialize_tuple(3)?;
            tuple.serialize_element(self.protocol())?;
            tuple.serialize_element(self.name())?;
            tuple.serialize_element(self.as_serialize())?;
            tuple.end()
        }
    }
}

impl<'de> Deserialize<'de> for AnyMessage {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: de::Deserializer<'de>,
    {
        // We don't deserialize dumps, so we can assume it's a tuple.
        deserializer.deserialize_tuple(3, AnyMessageDeserializeVisitor)
    }
}

struct AnyMessageDeserializeVisitor;

impl<'de> de::Visitor<'de> for AnyMessageDeserializeVisitor {
    type Value = AnyMessage;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "tuple of 3 elements")
    }

    #[inline]
    fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
    where
        A: de::SeqAccess<'de>,
    {
        let protocol = de::SeqAccess::next_element::<&str>(&mut seq)?
            .ok_or(de::Error::invalid_length(0usize, &"tuple of 3 elements"))?;

        let name = de::SeqAccess::next_element::<&str>(&mut seq)?
            .ok_or(de::Error::invalid_length(1usize, &"tuple of 3 elements"))?;

        de::SeqAccess::next_element_seed(&mut seq, MessageTag { protocol, name })?
            .ok_or(de::Error::invalid_length(2usize, &"tuple of 3 elements"))
    }
}

struct MessageTag<'a> {
    protocol: &'a str,
    name: &'a str,
}

impl<'de, 'tag> de::DeserializeSeed<'de> for MessageTag<'tag> {
    type Value = AnyMessage;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: de::Deserializer<'de>,
    {
        let Self { protocol, name } = self;

        let vtable = MessageVTable::lookup(protocol, name)
            .ok_or_else(|| de::Error::custom(format_args!("unknown message: {protocol}/{name}")))?;

        let out_ptr = alloc_repr(vtable);

        let mut deserializer = <dyn erased_serde::Deserializer<'_>>::erase(deserializer);
        // SAFETY: `out_ptr` belongs to the same object as the vtable.
        unsafe { (vtable.deserialize_any)(&mut deserializer, out_ptr) }
            .map_err(de::Error::custom)?;

        Ok(AnyMessage(out_ptr))
    }
}

cfg_network!({
    use rmp_serde::{decode, encode};

    impl AnyMessage {
        #[doc(hidden)]
        #[inline]
        pub fn read_msgpack(
            buffer: &[u8],
            protocol: &str,
            name: &str,
        ) -> Result<Option<Self>, decode::Error> {
            let Some(vtable) = MessageVTable::lookup(protocol, name) else {
                return Ok(None);
            };

            let out_ptr = alloc_repr(vtable);

            // SAFETY: `out_ptr` belongs to the same object as the vtable.
            unsafe { (vtable.read_msgpack)(buffer, out_ptr) }?;

            Ok(Some(Self(out_ptr)))
        }

        #[doc(hidden)]
        #[inline]
        pub fn write_msgpack(&self, out: &mut Vec<u8>, limit: usize) -> Result<(), encode::Error> {
            let vtable = self._vtable();
            // SAFETY: the vtable belongs to `self`.
            unsafe { (vtable.write_msgpack)(self.0, out, limit) }
        }
    }
});

// === AnyMessageRef ===

/// A reference to the message inside [`Envelope`] or [`AnyMessage`].
///
/// [`Envelope`]: crate::Envelope
pub struct AnyMessageRef<'a> {
    inner: ManuallyDrop<AnyMessage>, // never drop, borrows memory
    marker: PhantomData<&'a AnyMessage>,
}

impl<'a> AnyMessageRef<'a> {
    /// # Safety
    ///
    /// `ptr` must be a valid pointer for reads.
    pub(crate) unsafe fn new(ptr: NonNull<MessageRepr>) -> Self {
        Self {
            inner: ManuallyDrop::new(AnyMessage(ptr)),
            marker: PhantomData,
        }
    }

    #[inline]
    pub fn downcast_ref<M: Message>(&self) -> Option<&'a M> {
        let ret = self.inner.downcast_ref();

        // SAFETY: we produce lifetime bound to the original one.
        // Note: semantically it's shortening not extending.
        unsafe { mem::transmute::<Option<&M>, Option<&'a M>>(ret) }
    }

    pub(crate) unsafe fn downcast_ref_unchecked<M: Message>(&self) -> &'a M {
        let ret = self.inner.downcast_ref_unchecked();

        // SAFETY: we produce lifetime bound to the original one.
        // Note: semantically it's shortening not extending.
        unsafe { mem::transmute::<&M, &'a M>(ret) }
    }
}

impl Deref for AnyMessageRef<'_> {
    type Target = AnyMessage;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl fmt::Debug for AnyMessageRef<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        (**self).fmt(f)
    }
}

impl Serialize for AnyMessageRef<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: ser::Serializer,
    {
        (**self).serialize(serializer)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::{message, scope::SerdeMode};

    #[message]
    #[derive(PartialEq)]
    struct Unused;

    #[message]
    #[derive(PartialEq)]
    struct P0;

    #[message]
    #[derive(PartialEq)]
    struct P1(u8);

    #[message]
    #[derive(PartialEq)]
    struct P8(u64);

    #[message]
    #[derive(PartialEq)]
    struct P16(u128);

    fn check_basic_ops<M: Message + PartialEq>(message: M) {
        let message_box = AnyMessage::new(message.clone());

        // Debug
        assert_eq!(format!("{:?}", message_box), format!("{:?}", message));
        assert_eq!(
            format!("{:?}", message_box.as_ref()),
            format!("{:?}", message)
        );

        // Clone
        let message_box_2 = message_box.clone();
        assert_eq!(message_box_2.downcast::<M>().unwrap(), message);
        let message_box_3 = message_box.as_ref().clone();
        assert_eq!(message_box_3.downcast_ref::<M>().unwrap(), &message);

        // AnyMessage -> AnyMessage
        let message_box_2 = AnyMessage::new(message_box_3);
        assert_eq!(message_box_2.downcast::<M>().unwrap(), message);

        // Downcast
        assert!(message_box.is::<M>());
        assert!(message_box.as_ref().is::<M>());
        assert!(!message_box.is::<Unused>());
        assert!(!message_box.as_ref().is::<Unused>());
        assert_eq!(message_box.downcast_ref::<M>(), Some(&message));
        assert_eq!(message_box.as_ref().downcast_ref::<M>(), Some(&message));
        assert_eq!(message_box.downcast_ref::<Unused>(), None);
        assert_eq!(message_box.as_ref().downcast_ref::<Unused>(), None);

        let message_box = message_box.downcast::<Unused>().unwrap_err();
        assert_eq!(message_box.downcast::<M>().unwrap(), message);
    }

    #[test]
    fn basic_ops_miri() {
        check_basic_ops(P0);
        check_basic_ops(P1(42));
        check_basic_ops(P8(424242));
        check_basic_ops(P16(424242424242));
    }

    #[message]
    struct WithImplicitDrop(Arc<()>);

    #[test]
    fn drop_miri() {
        let counter = Arc::new(());
        let message = WithImplicitDrop(counter.clone());

        assert_eq!(Arc::strong_count(&counter), 2);
        let message_box = AnyMessage::new(message);
        assert_eq!(Arc::strong_count(&counter), 2);
        let message_box_2 = message_box.clone();
        let message_box_3 = message_box.clone();
        assert_eq!(Arc::strong_count(&counter), 4);

        drop(message_box_2);
        assert_eq!(Arc::strong_count(&counter), 3);
        drop(message_box);
        assert_eq!(Arc::strong_count(&counter), 2);
        drop(message_box_3);
        assert_eq!(Arc::strong_count(&counter), 1);
    }

    #[message]
    #[derive(PartialEq)]
    struct MyCoolMessage {
        field_a: u32,
        field_b: String,
        field_c: f64,
    }

    impl MyCoolMessage {
        fn example() -> Self {
            Self {
                field_a: 123,
                field_b: String::from("Hello world"),
                field_c: 0.5,
            }
        }
    }

    #[test]
    fn serialize_miri() {
        let any_msg = AnyMessage::new(MyCoolMessage::example());
        for mode in [SerdeMode::Normal, SerdeMode::Network] {
            let dump =
                crate::scope::with_serde_mode(mode, || serde_json::to_string(&any_msg).unwrap());
            assert_eq!(
                dump,
                r#"["elfo-core","MyCoolMessage",{"field_a":123,"field_b":"Hello world","field_c":0.5}]"#
            );
        }

        let dump = crate::scope::with_serde_mode(SerdeMode::Dumping, || {
            serde_json::to_string(&any_msg).unwrap()
        });
        assert_eq!(
            dump,
            r#"{"protocol":"elfo-core","name":"MyCoolMessage","payload":{"field_a":123,"field_b":"Hello world","field_c":0.5}}"#
        );
    }

    #[test]
    fn serde_roundtrip() {
        let msg = MyCoolMessage::example();
        let any_msg = AnyMessage::new(msg.clone());
        let serialized = serde_json::to_string(&any_msg).unwrap();

        let deserialized_any_msg: AnyMessage = serde_json::from_str(&serialized).unwrap();
        let deserialized_msg: MyCoolMessage = deserialized_any_msg.downcast().unwrap();

        assert_eq!(msg, deserialized_msg);
    }
}
