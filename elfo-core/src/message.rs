use std::{
    alloc, fmt,
    marker::PhantomData,
    mem::{self, ManuallyDrop},
    ops::Deref,
    ptr::{self, NonNull},
};

use fxhash::{FxHashMap, FxHashSet};
use linkme::distributed_slice;
use metrics::Label;
use once_cell::sync::Lazy; // TODO: replace with std?
use serde::{
    de,
    ser::{self, SerializeStruct as _, SerializeTuple as _},
    Deserialize, Serialize,
};
use smallbox::smallbox;

use crate::{dumping, scope::SerdeMode};

// === Message ===

pub trait Message:
    fmt::Debug + Clone + Send + Serialize + for<'de> Deserialize<'de> + 'static
{
    #[inline(always)]
    fn name(&self) -> &'static str {
        self._vtable().name
    }

    #[inline(always)]
    fn protocol(&self) -> &'static str {
        self._vtable().protocol
    }

    #[doc(hidden)]
    #[inline(always)]
    fn labels(&self) -> &'static [Label] {
        self._vtable().labels
    }

    #[doc(hidden)]
    #[inline(always)]
    fn dumping_allowed(&self) -> bool {
        self._vtable().dumping_allowed
    }

    #[deprecated(note = "use `AnyMessage::new` instead")]
    #[doc(hidden)]
    #[inline(always)]
    fn upcast(self) -> AnyMessage {
        self._into_any()
    }

    // Private API.

    #[doc(hidden)]
    fn _type_id() -> MessageTypeId;

    #[doc(hidden)]
    fn _vtable(&self) -> &'static MessageVTable;

    #[doc(hidden)]
    #[inline(always)]
    fn _can_get_from(type_id: MessageTypeId) -> bool {
        Self::_type_id() == type_id
    }

    // Called in `_read()` and `_write()` to avoid
    // * [rust#47384](https://github.com/rust-lang/rust/issues/47384)
    // * [rust#99721](https://github.com/rust-lang/rust/issues/99721)
    #[doc(hidden)]
    fn _touch(&self);

    #[doc(hidden)]
    #[inline(always)]
    fn _into_any(self) -> AnyMessage {
        AnyMessage::from_real(self)
    }

    #[doc(hidden)]
    #[inline(always)]
    fn _erase(&self) -> dumping::ErasedMessage {
        smallbox!(self.clone())
    }

    #[doc(hidden)]
    #[inline(always)]
    fn _repr_layout(&self) -> alloc::Layout {
        self._vtable().repr_layout

        // let layout = alloc::Layout::new::<MessageRepr<Self>>();
        // TODO: debug_assert_eq!(self._vtable().repr_layout, layout);
        // where to check it?
    }

    #[doc(hidden)]
    #[inline(always)]
    unsafe fn _read(ptr: NonNull<MessageRepr>) -> Self {
        let data_ref = unsafe { &ptr.cast::<MessageRepr<Self>>().as_ref().data };
        let data = unsafe { ptr::read(data_ref) };
        data._touch();
        data
    }

    #[doc(hidden)]
    #[inline(always)]
    unsafe fn _write(self, ptr: NonNull<MessageRepr>) {
        self._touch();
        let repr = MessageRepr::new(self);
        unsafe { ptr::write(ptr.cast::<MessageRepr<Self>>().as_ptr(), repr) };
    }
}

// === Request ===

pub trait Request: Message {
    type Response: fmt::Debug + Clone + Send + Serialize; // TODO

    #[doc(hidden)]
    type Wrapper: Message + Into<Self::Response> + From<Self::Response>;
}

// === MessageTypeId ===

#[derive(Clone, Copy, Debug)]
pub struct MessageTypeId(*const ());

unsafe impl Send for MessageTypeId {}
unsafe impl Sync for MessageTypeId {}

impl MessageTypeId {
    #[inline]
    pub const fn new(vtable: &'static MessageVTable) -> Self {
        Self(vtable as *const _ as *const ())
    }
}

impl PartialEq for MessageTypeId {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        ptr::eq(self.0, other.0)
    }
}

// === MessageRepr ===

#[derive(Clone)]
#[repr(C)]
pub struct MessageRepr<M = ()> {
    vtable: &'static MessageVTable,
    data: M,
}

impl<M> MessageRepr<M>
where
    M: Message,
{
    pub(crate) fn new(message: M) -> Self {
        debug_assert_ne!(M::_type_id(), AnyMessage::_type_id());

        Self {
            vtable: message._vtable(),
            data: message,
        }
    }
}

// === AnyMessage ===

pub struct AnyMessage(NonNull<MessageRepr>);

assert_not_impl_any!(AnyMessage: Sync);

unsafe impl Send for AnyMessage {}

// TODO: Pin/Unpin?
// TODO: miri strict sptr

impl AnyMessage {
    pub fn new<M: Message>(message: M) -> Self {
        message._into_any()
    }

    fn from_real<M: Message>(message: M) -> Self {
        let ptr = unsafe { alloc_repr(message._vtable()) };
        unsafe { message._write(ptr) };
        Self(ptr)
    }

    #[inline]
    pub fn type_id(&self) -> MessageTypeId {
        MessageTypeId::new(self._vtable())
    }

    #[inline]
    pub fn is<M: Message>(&self) -> bool {
        M::_can_get_from(self.type_id())
    }

    #[inline]
    pub fn downcast_ref<M: Message>(&self) -> Option<&M> {
        self.is::<M>()
            .then(|| unsafe { self.downcast_ref_unchecked() })
    }

    pub(crate) unsafe fn downcast_ref_unchecked<M: Message>(&self) -> &M {
        // TODO: support `AnyMessage`
        debug_assert_ne!(M::_type_id(), Self::_type_id());

        &self.0.cast::<MessageRepr<M>>().as_ref().data
    }

    #[inline]
    pub fn downcast<M: Message>(self) -> Result<M, AnyMessage> {
        if !self.is::<M>() {
            return Err(self);
        }

        let data = unsafe { M::_read(self.0) };

        unsafe { dealloc_repr(self.0) };

        mem::forget(self);

        Ok(data)
    }

    pub(crate) unsafe fn clone_into(&self, out_ptr: NonNull<MessageRepr>) {
        let vtable = self._vtable();
        (vtable.clone)(self.0, out_ptr);
    }

    pub(crate) unsafe fn drop_in_place(&self) {
        let vtable = self._vtable();
        (vtable.drop)(self.0);
    }
}

unsafe fn alloc_repr(vtable: &'static MessageVTable) -> NonNull<MessageRepr> {
    let ptr = alloc::alloc(vtable.repr_layout);

    let Some(ptr) = NonNull::new(ptr) else {
        alloc::handle_alloc_error(vtable.repr_layout);
    };

    ptr.cast()
}

unsafe fn dealloc_repr(ptr: NonNull<MessageRepr>) {
    let ptr = ptr.as_ptr();
    let vtable = (*ptr).vtable;

    alloc::dealloc(ptr.cast(), vtable.repr_layout);
}

impl Drop for AnyMessage {
    fn drop(&mut self) {
        unsafe { self.drop_in_place() };

        unsafe { dealloc_repr(self.0) };
    }
}

impl Clone for AnyMessage {
    fn clone(&self) -> Self {
        let out_ptr = unsafe { alloc_repr(self._vtable()) };

        unsafe { self.clone_into(out_ptr) };

        Self(out_ptr)
    }
}

impl fmt::Debug for AnyMessage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        unsafe { (self._vtable().debug)(self.0, f) }
    }
}

impl Message for AnyMessage {
    #[inline(always)]
    fn _type_id() -> MessageTypeId {
        MessageTypeId::new(&VTABLE_STUB)
    }

    #[inline(always)]
    fn _vtable(&self) -> &'static MessageVTable {
        unsafe { (*self.0.as_ptr()).vtable }
    }

    #[inline(always)]
    fn _can_get_from(_: MessageTypeId) -> bool {
        true
    }

    #[inline(always)]
    fn _touch(&self) {}

    #[inline(always)]
    fn _into_any(self) -> AnyMessage {
        self
    }

    #[inline(always)]
    fn _erase(&self) -> dumping::ErasedMessage {
        let vtable = self._vtable();
        unsafe { (vtable.erase)(self.0) }
    }

    #[inline(always)]
    unsafe fn _read(ptr: NonNull<MessageRepr>) -> Self {
        let vtable = (*ptr.as_ptr()).vtable;
        let this = unsafe { alloc_repr(vtable) };

        unsafe {
            ptr::copy_nonoverlapping(
                ptr.cast::<u8>().as_ptr(),
                this.cast::<u8>().as_ptr(),
                vtable.repr_layout.size(),
            )
        };

        Self(this)
    }

    #[inline(always)]
    unsafe fn _write(self, out_ptr: NonNull<MessageRepr>) {
        unsafe {
            ptr::copy_nonoverlapping(
                self.0.cast::<u8>().as_ptr(),
                out_ptr.cast::<u8>().as_ptr(),
                self._vtable().repr_layout.size(),
            )
        };

        unsafe { dealloc_repr(self.0) };

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
        // TODO: avoid allocation here (add `_erase_ref`)
        let erased_msg = self._erase();

        // TODO: use compact form only for network?
        if crate::scope::serde_mode() == SerdeMode::Dumping {
            let mut fields = serializer.serialize_struct("AnyMessage", 3)?;
            fields.serialize_field("protocol", self.protocol())?;
            fields.serialize_field("name", self.name())?;
            fields.serialize_field("payload", &*erased_msg)?;
            fields.end()
        } else {
            let mut tuple = serializer.serialize_tuple(3)?;
            tuple.serialize_element(self.protocol())?;
            tuple.serialize_element(self.name())?;
            tuple.serialize_element(&*erased_msg)?;
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

        let vtable = lookup_vtable(protocol, name)
            .ok_or_else(|| de::Error::custom(format_args!("unknown message: {protocol}/{name}")))?;

        let out_ptr = unsafe { alloc_repr(vtable) };

        let mut deserializer = <dyn erased_serde::Deserializer<'_>>::erase(deserializer);
        unsafe { (vtable.deserialize_any)(&mut deserializer, out_ptr) }
            .map_err(de::Error::custom)?;

        Ok(AnyMessage(out_ptr))
    }
}

cfg_network!({
    use std::io;

    use rmp_serde::{decode, encode};

    impl AnyMessage {
        #[doc(hidden)]
        #[inline]
        pub fn read_msgpack(
            buffer: &[u8],
            protocol: &str,
            name: &str,
        ) -> Result<Option<Self>, decode::Error> {
            let Some(vtable) = lookup_vtable(protocol, name) else {
                return Ok(None);
            };

            let out_ptr = unsafe { alloc_repr(vtable) };

            unsafe { (vtable.read_msgpack)(buffer, out_ptr) }?;

            Ok(Some(Self(out_ptr)))
        }

        #[doc(hidden)]
        #[inline]
        pub fn write_msgpack(
            &self,
            buffer: &mut Vec<u8>,
            limit: usize,
        ) -> Result<(), encode::Error> {
            let vtable = self._vtable();
            let out = LimitedWrite(buffer, limit);
            unsafe { (vtable.write_msgpack)(self.0, out) }
        }
    }

    // The compiler requires all arguments to be visible.
    pub struct LimitedWrite<W>(W, usize);

    impl<W: io::Write> io::Write for LimitedWrite<W> {
        #[inline]
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            if buf.len() > self.1 {
                self.1 = 0;
                return Ok(0);
            }

            self.1 -= buf.len();
            self.0.write(buf)
        }

        #[inline]
        fn flush(&mut self) -> io::Result<()> {
            self.0.flush()
        }
    }
});

// === AnyMessageRef ===

// TODO: method to get AnyMessageRef from AnyMessage

pub struct AnyMessageRef<'a> {
    inner: ManuallyDrop<AnyMessage>, // never drop, borrows memory
    marker: PhantomData<&'a AnyMessage>,
}

impl<'a> AnyMessageRef<'a> {
    pub(crate) unsafe fn new(ptr: NonNull<MessageRepr>) -> Self {
        Self {
            inner: ManuallyDrop::new(AnyMessage(ptr)),
            marker: PhantomData,
        }
    }

    #[inline]
    pub fn downcast_ref<M: Message>(&self) -> Option<&'a M> {
        self.is::<M>()
            .then(|| unsafe { self.downcast_ref_unchecked() })
    }

    pub(crate) unsafe fn downcast_ref_unchecked<M: Message>(&self) -> &'a M {
        &self.inner.0.cast::<MessageRepr<M>>().as_ref().data
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

// === ProtocolExtractor ===
// See https://github.com/GoldsteinE/gh-blog/blob/master/const_deref_specialization/src/lib.md

// Reexported in `elfo::_priv`.
pub struct ProtocolExtractor;

// Reexported in `elfo::_priv`.
pub trait ProtocolHolder {
    const PROTOCOL: Option<&'static str>;
}

// Reexported in `elfo::_priv`.
pub struct DefaultProtocolHolder;

impl ProtocolHolder for DefaultProtocolHolder {
    // `None` means a crate's name is used.
    const PROTOCOL: Option<&'static str> = None;
}

impl Deref for ProtocolExtractor {
    type Target = DefaultProtocolHolder;

    fn deref(&self) -> &Self::Target {
        &DefaultProtocolHolder
    }
}

impl DefaultProtocolHolder {
    pub fn holder(&self) -> Self {
        Self
    }
}

// === MessageVTable ===

// Reexported in `elfo::_priv`.
/// Message Virtual Table.
pub struct MessageVTable {
    pub repr_layout: alloc::Layout, // of `MessageRepr<M>`
    pub name: &'static str,
    pub protocol: &'static str,
    pub labels: &'static [Label],
    pub dumping_allowed: bool, // TODO: introduce `DumpingMode`.
    // TODO: field ordering (better for cache)
    // TODO:
    // pub deserialize_any: fn(&mut dyn erased_serde::Deserializer<'_>) -> Result<AnyMessage,
    // erased_serde::Error>,
    #[cfg(feature = "network")]
    pub read_msgpack: unsafe fn(&[u8], NonNull<MessageRepr>) -> Result<(), decode::Error>,
    #[cfg(feature = "network")]
    #[allow(clippy::type_complexity)]
    pub write_msgpack:
        unsafe fn(NonNull<MessageRepr>, LimitedWrite<&mut Vec<u8>>) -> Result<(), encode::Error>,
    pub debug: unsafe fn(NonNull<MessageRepr>, &mut fmt::Formatter<'_>) -> fmt::Result,
    pub clone: unsafe fn(NonNull<MessageRepr>, NonNull<MessageRepr>),
    pub erase: unsafe fn(NonNull<MessageRepr>) -> dumping::ErasedMessage,
    pub deserialize_any: unsafe fn(
        deserializer: &mut dyn erased_serde::Deserializer<'_>,
        out_ptr: NonNull<MessageRepr>,
    ) -> Result<(), erased_serde::Error>,
    pub drop: unsafe fn(NonNull<MessageRepr>),
}

// TODO: rename
static VTABLE_STUB: MessageVTable = MessageVTable {
    repr_layout: alloc::Layout::new::<()>(),
    name: "",
    protocol: "",
    labels: &[],
    dumping_allowed: false,
    #[cfg(feature = "network")]
    read_msgpack: |_, _| unreachable!(),
    #[cfg(feature = "network")]
    write_msgpack: |_, _| unreachable!(),
    debug: |_, _| unreachable!(),
    clone: |_, _| unreachable!(),
    erase: |_| unreachable!(),
    deserialize_any: |_, _| unreachable!(),
    drop: |_| unreachable!(),
};

// For monomorphization in the `#[message]` macro.
// Reeexported in `elfo::_priv`.
pub mod vtablefns {
    use super::*;

    pub unsafe fn drop<M>(ptr: NonNull<MessageRepr>) {
        ptr::drop_in_place(ptr.cast::<MessageRepr<M>>().as_ptr());
    }

    pub unsafe fn clone<M: Clone>(ptr: NonNull<MessageRepr>, out_ptr: NonNull<MessageRepr>) {
        ptr::write(
            out_ptr.cast::<MessageRepr<M>>().as_ptr(),
            ptr.cast::<MessageRepr<M>>().as_ref().clone(),
        );
    }

    pub unsafe fn debug<M: fmt::Debug>(
        ptr: NonNull<MessageRepr>,
        f: &mut fmt::Formatter<'_>,
    ) -> fmt::Result {
        let data = &ptr.cast::<MessageRepr<M>>().as_ref().data;
        fmt::Debug::fmt(data, f)
    }

    pub unsafe fn erase<M: Message>(ptr: NonNull<MessageRepr>) -> dumping::ErasedMessage {
        let data = ptr.cast::<MessageRepr<M>>().as_ref().data.clone();
        smallbox!(data)
    }

    pub unsafe fn deserialize_any<M: Message>(
        deserializer: &mut dyn erased_serde::Deserializer<'_>,
        out_ptr: NonNull<MessageRepr>,
    ) -> Result<(), erased_serde::Error> {
        let data = erased_serde::deserialize::<M>(deserializer)?;
        ptr::write(
            out_ptr.cast::<MessageRepr<M>>().as_ptr(),
            MessageRepr::new(data),
        );
        Ok(())
    }

    cfg_network!({
        pub unsafe fn read_msgpack<M: Message>(
            buffer: &[u8],
            out_ptr: NonNull<MessageRepr>,
        ) -> Result<(), decode::Error> {
            let data = decode::from_slice(buffer)?;
            ptr::write(
                out_ptr.cast::<MessageRepr<M>>().as_ptr(),
                MessageRepr::new(data),
            );
            Ok(())
        }

        pub unsafe fn write_msgpack<M: Message>(
            ptr: NonNull<MessageRepr>,
            mut out: LimitedWrite<&mut Vec<u8>>,
        ) -> Result<(), encode::Error> {
            let data = &ptr.cast::<MessageRepr<M>>().as_ref().data;
            encode::write_named(&mut out, data)
        }
    });
}

// Reexported in `elfo::_priv`.
#[distributed_slice]
pub static MESSAGE_VTABLES_LIST: [&'static MessageVTable] = [..];

static MESSAGE_VTABLES_MAP: Lazy<FxHashMap<(&'static str, &'static str), &'static MessageVTable>> =
    Lazy::new(|| {
        MESSAGE_VTABLES_LIST
            .iter()
            .map(|vtable| ((vtable.protocol, vtable.name), *vtable))
            .collect()
    });

fn lookup_vtable(protocol: &str, name: &str) -> Option<&'static MessageVTable> {
    // Extend lifetimes to static in order to get `(&'static str, &'static str)`.
    // SAFETY: this pair doesn't overlive the function.
    let (protocol, name) = unsafe {
        (
            std::mem::transmute::<&str, &'static str>(protocol),
            std::mem::transmute::<&str, &'static str>(name),
        )
    };

    MESSAGE_VTABLES_MAP.get(&(protocol, name)).copied()
}

pub(crate) fn check_uniqueness() -> Result<(), Vec<(String, String)>> {
    if MESSAGE_VTABLES_MAP.len() == MESSAGE_VTABLES_LIST.len() {
        return Ok(());
    }

    Err(MESSAGE_VTABLES_LIST
        .iter()
        .filter(|vtable| {
            let stored = lookup_vtable(vtable.protocol, vtable.name).unwrap();
            MessageTypeId::new(stored) != MessageTypeId::new(vtable)
        })
        .map(|vtable| (vtable.protocol.to_string(), vtable.name.to_string()))
        .collect::<FxHashSet<_>>()
        .into_iter()
        .collect::<Vec<_>>())
}

// TODO: add tests for `AnyMessageRef`

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

    fn check_ops<M: Message + PartialEq>(message: M) {
        let message_box = AnyMessage::new(message.clone());

        // Debug
        assert_eq!(format!("{:?}", message_box), format!("{:?}", message));

        // Clone
        let message_box_2 = message_box.clone();
        assert_eq!(message_box_2.downcast::<M>().unwrap(), message);

        // Downcast
        assert!(message_box.is::<M>());
        assert!(!message_box.is::<Unused>());
        assert_eq!(message_box.downcast_ref::<M>(), Some(&message));
        assert_eq!(message_box.downcast_ref::<Unused>(), None);

        let message_box = message_box.downcast::<Unused>().unwrap_err();
        assert_eq!(message_box.downcast::<M>().unwrap(), message);
    }

    #[test]
    fn miri_ops() {
        check_ops(P0);
        check_ops(P1(42));
        check_ops(P8(424242));
        check_ops(P16(424242424242));
    }

    #[message]
    struct WithImplicitDrop(Arc<()>);

    #[test]
    fn miri_drop() {
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
    fn any_message_deserialize() {
        let msg = MyCoolMessage::example();
        let any_msg = AnyMessage::new(msg.clone());
        let serialized = serde_json::to_string(&any_msg).unwrap();

        let deserialized_any_msg: AnyMessage = serde_json::from_str(&serialized).unwrap();
        let deserialized_msg: MyCoolMessage = deserialized_any_msg.downcast().unwrap();

        assert_eq!(msg, deserialized_msg);
    }

    #[test]
    fn any_message_serialize() {
        let any_msg = AnyMessage::new(MyCoolMessage::example());
        for mode in [SerdeMode::Normal, SerdeMode::Network] {
            let dump =
                crate::scope::with_serde_mode(mode, || serde_json::to_string(&any_msg).unwrap());
            assert_eq!(
                dump,
                r#"["elfo-core","MyCoolMessage",{"field_a":123,"field_b":"Hello world","field_c":0.5}]"#
            );
        }
    }

    #[test]
    fn any_message_dump() {
        let any_msg = AnyMessage::new(MyCoolMessage::example());
        let dump = crate::scope::with_serde_mode(SerdeMode::Dumping, || {
            serde_json::to_string(&any_msg).unwrap()
        });
        assert_eq!(
            dump,
            r#"{"protocol":"elfo-core","name":"MyCoolMessage","payload":{"field_a":123,"field_b":"Hello world","field_c":0.5}}"#
        );
    }
}
