use std::{any::Any, fmt, ops::Deref};

use fxhash::{FxHashMap, FxHashSet};
use linkme::distributed_slice;
use metrics::Label;
use once_cell::sync::Lazy;
use serde::{
    de::{DeserializeSeed, SeqAccess, Visitor},
    ser::{SerializeStruct as _, SerializeTuple as _},
    Deserialize, Deserializer, Serialize,
};
use smallbox::{smallbox, SmallBox};

use elfo_utils::unlikely;

use crate::{dumping, scope::SerdeMode};

pub trait Message: fmt::Debug + Clone + Any + Send + Serialize + for<'de> Deserialize<'de> {
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

    #[doc(hidden)]
    #[inline(always)]
    fn upcast(self) -> AnyMessage {
        self._touch();
        AnyMessage {
            vtable: self._vtable(),
            data: smallbox!(self),
        }
    }

    // Private API.

    #[doc(hidden)]
    fn _vtable(&self) -> &'static MessageVTable;

    // Called while upcasting/downcasting to avoid
    // [rust#47384](https://github.com/rust-lang/rust/issues/47384).
    #[doc(hidden)]
    fn _touch(&self);

    #[doc(hidden)]
    #[inline(always)]
    fn _erase(&self) -> dumping::ErasedMessage {
        smallbox!(self.clone())
    }
}

pub trait Request: Message {
    type Response: fmt::Debug + Clone + Send + Serialize;

    #[doc(hidden)]
    type Wrapper: Message + Into<Self::Response> + From<Self::Response>;
}

// === AnyMessage ===

// Reexported in `elfo::_priv`.
pub struct AnyMessage {
    vtable: &'static MessageVTable,
    data: SmallBox<dyn Any + Send, [usize; 24]>,
}

impl AnyMessage {
    #[inline]
    pub fn is<M: Message>(&self) -> bool {
        self.data.is::<M>()
    }

    #[inline]
    pub fn type_id(&self) -> std::any::TypeId {
        (*self.data).type_id()
    }

    #[inline]
    pub fn downcast_ref<M: Message>(&self) -> Option<&M> {
        self.data.downcast_ref::<M>().map(|message| {
            message._touch();
            message
        })
    }

    #[inline]
    pub fn downcast<M: Message>(self) -> Result<M, AnyMessage> {
        if !self.is::<M>() {
            return Err(self);
        }

        let message = self
            .data
            .downcast::<M>()
            .expect("cannot downcast")
            .into_inner();

        message._touch();
        Ok(message)
    }
}

impl Message for AnyMessage {
    #[inline(always)]
    fn upcast(self) -> AnyMessage {
        self
    }

    #[inline(always)]
    fn _vtable(&self) -> &'static MessageVTable {
        self.vtable
    }

    #[inline(always)]
    fn _touch(&self) {}

    #[doc(hidden)]
    #[inline(always)]
    fn _erase(&self) -> dumping::ErasedMessage {
        (self.vtable.erase)(self)
    }
}

impl Clone for AnyMessage {
    #[inline]
    fn clone(&self) -> Self {
        (self.vtable.clone)(self)
    }
}

impl fmt::Debug for AnyMessage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        (self.vtable.debug)(self, f)
    }
}

// `Serialize` / `Deserialize` impls for `AnyMessage` are not used when sending
// it by itself, only when it's used in other messages.
impl Serialize for AnyMessage {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::ser::Serializer,
    {
        // TODO: avoid allocation here
        let erased_msg = self._erase();
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
        D: serde::de::Deserializer<'de>,
    {
        // We don't deserialize dumps, so we can assume it's a tuple.
        deserializer.deserialize_tuple(3, AnyMessageDeserializeVisitor)
    }
}

struct AnyMessageDeserializeVisitor;

impl<'de> Visitor<'de> for AnyMessageDeserializeVisitor {
    type Value = AnyMessage;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "tuple of 3 elements")
    }

    #[inline]
    fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let protocol = serde::de::SeqAccess::next_element::<&str>(&mut seq)?.ok_or(
            serde::de::Error::invalid_length(0usize, &"tuple of 3 elements"),
        )?;

        let name = serde::de::SeqAccess::next_element::<&str>(&mut seq)?.ok_or(
            serde::de::Error::invalid_length(1usize, &"tuple of 3 elements"),
        )?;

        serde::de::SeqAccess::next_element_seed(&mut seq, MessageTag { protocol, name })?.ok_or(
            serde::de::Error::invalid_length(2usize, &"tuple of 3 elements"),
        )
    }
}

struct MessageTag<'a> {
    protocol: &'a str,
    name: &'a str,
}

impl<'de, 'tag> DeserializeSeed<'de> for MessageTag<'tag> {
    type Value = AnyMessage;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        let deserialize_any = lookup_vtable(self.protocol, self.name)
            .ok_or(serde::de::Error::custom(
                "unknown protocol/name combination",
            ))?
            .deserialize_any;

        let mut deserializer = <dyn erased_serde::Deserializer<'_>>::erase(deserializer);
        deserialize_any(&mut deserializer).map_err(serde::de::Error::custom)
    }
}

cfg_network!({
    use rmp_serde as rmps;

    impl AnyMessage {
        #[doc(hidden)]
        #[inline]
        pub fn read_msgpack(
            buffer: &[u8],
            protocol: &str,
            name: &str,
        ) -> Result<Option<Self>, rmps::decode::Error> {
            lookup_vtable(protocol, name)
                .map(|vtable| (vtable.read_msgpack)(buffer))
                .transpose()
        }

        #[doc(hidden)]
        #[inline]
        pub fn write_msgpack(
            &self,
            buffer: &mut Vec<u8>,
            limit: usize,
        ) -> Result<(), rmps::encode::Error> {
            (self.vtable.write_msgpack)(self, buffer, limit)
        }
    }

    // For monomorphization in the `#[message]` macro.
    // Reexported in `elfo::_priv`.
    #[inline]
    pub fn read_msgpack<M: Message>(buffer: &[u8]) -> Result<M, rmps::decode::Error> {
        rmps::decode::from_slice(buffer)
    }

    // For monomorphization in the `#[message]` macro.
    // Reexported in `elfo::_priv`.
    #[inline]
    pub fn write_msgpack(
        buffer: &mut Vec<u8>,
        limit: usize,
        message: &impl Message,
    ) -> Result<(), rmps::encode::Error> {
        let mut wr = LimitedWrite(buffer, limit);
        rmps::encode::write_named(&mut wr, message)
    }

    struct LimitedWrite<W>(W, usize);

    impl<W: std::io::Write> std::io::Write for LimitedWrite<W> {
        #[inline]
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            if unlikely(buf.len() > self.1) {
                self.1 = 0;
                return Ok(0);
            }

            self.1 -= buf.len();
            self.0.write(buf)
        }

        #[inline]
        fn flush(&mut self) -> std::io::Result<()> {
            self.0.flush()
        }
    }
});

// === ProtocolExtractor ===
// Reexported in `elfo::_priv`.
// See https://github.com/GoldsteinE/gh-blog/blob/master/const_deref_specialization/src/lib.md

pub struct ProtocolExtractor;

pub trait ProtocolHolder {
    const PROTOCOL: Option<&'static str>;
}

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
    /// Just a message's name.
    pub name: &'static str,
    /// A protocol's name.
    /// Usually, it's a crate name where the message is defined.
    pub protocol: &'static str,
    pub labels: &'static [Label],
    pub dumping_allowed: bool, // TODO: introduce `DumpingMode`.
    pub clone: fn(&AnyMessage) -> AnyMessage,
    pub debug: fn(&AnyMessage, &mut fmt::Formatter<'_>) -> fmt::Result,
    pub erase: fn(&AnyMessage) -> dumping::ErasedMessage,
    pub deserialize_any:
        fn(&mut dyn erased_serde::Deserializer<'_>) -> Result<AnyMessage, erased_serde::Error>,
    #[cfg(feature = "network")]
    pub write_msgpack: fn(&AnyMessage, &mut Vec<u8>, usize) -> Result<(), rmps::encode::Error>,
    #[cfg(feature = "network")]
    pub read_msgpack: fn(&[u8]) -> Result<AnyMessage, rmps::decode::Error>,
}

// Reexported in `elfo::_priv`.
#[distributed_slice]
pub static MESSAGE_LIST: [&'static MessageVTable] = [..];

static MESSAGES: Lazy<FxHashMap<(&'static str, &'static str), &'static MessageVTable>> =
    Lazy::new(|| {
        MESSAGE_LIST
            .iter()
            .map(|vtable| ((vtable.protocol, vtable.name), *vtable))
            .collect()
    });

fn lookup_vtable(protocol: &str, name: &str) -> Option<&'static MessageVTable> {
    // Extend lifetimes to static in order to get `(&'static str, &'static str)`.
    // SAFETY: this pair doesn't overlive the function.
    let (protocol, name) = unsafe {
        (
            std::mem::transmute::<_, &'static str>(protocol),
            std::mem::transmute::<_, &'static str>(name),
        )
    };

    MESSAGES.get(&(protocol, name)).copied()
}

pub(crate) fn check_uniqueness() -> Result<(), Vec<(String, String)>> {
    if MESSAGES.len() == MESSAGE_LIST.len() {
        return Ok(());
    }

    fn vtable_eq(lhs: &'static MessageVTable, rhs: &'static MessageVTable) -> bool {
        std::ptr::eq(lhs, rhs)
    }

    Err(MESSAGE_LIST
        .iter()
        .filter(|vtable| {
            let stored = MESSAGES.get(&(vtable.protocol, vtable.name)).unwrap();
            !vtable_eq(stored, vtable)
        })
        .map(|vtable| (vtable.protocol.to_string(), vtable.name.to_string()))
        .collect::<FxHashSet<_>>()
        .into_iter()
        .collect::<Vec<_>>())
}

#[cfg(test)]
mod tests {
    use crate::{message, message::AnyMessage, scope::SerdeMode, Message};

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
        let any_msg = msg.clone().upcast();
        let serialized = serde_json::to_string(&any_msg).unwrap();

        let deserialized_any_msg: AnyMessage = serde_json::from_str(&serialized).unwrap();
        let deserialized_msg: MyCoolMessage = deserialized_any_msg.downcast().unwrap();

        assert_eq!(msg, deserialized_msg);
    }

    #[test]
    fn any_message_serialize() {
        let any_msg = MyCoolMessage::example().upcast();
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
        let any_msg = MyCoolMessage::example().upcast();
        let dump = crate::scope::with_serde_mode(SerdeMode::Dumping, || {
            serde_json::to_string(&any_msg).unwrap()
        });
        assert_eq!(
            dump,
            r#"{"protocol":"elfo-core","name":"MyCoolMessage","payload":{"field_a":123,"field_b":"Hello world","field_c":0.5}}"#
        );
    }
}
