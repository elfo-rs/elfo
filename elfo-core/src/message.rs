use std::{any::Any, fmt, ops::Deref};

use fxhash::FxHashMap;
use linkme::distributed_slice;
use metrics::Label;
use serde::{Deserialize, Serialize};
use smallbox::{smallbox, SmallBox};

use elfo_utils::unlikely;

use crate::dumping;

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

impl Serialize for AnyMessage {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::ser::Serializer,
    {
        // TODO: avoid allocation here
        self._erase().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for AnyMessage {
    fn deserialize<D>(_deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::de::Deserializer<'de>,
    {
        Err(serde::de::Error::custom(
            "AnyMessage cannot be deserialized",
        ))
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
#[derive(Clone)]
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
    #[cfg(feature = "network")]
    pub write_msgpack: fn(&AnyMessage, &mut Vec<u8>, usize) -> Result<(), rmps::encode::Error>,
    #[cfg(feature = "network")]
    pub read_msgpack: fn(&[u8]) -> Result<AnyMessage, rmps::decode::Error>,
}

// Reexported in `elfo::_priv`.
#[distributed_slice]
pub static MESSAGE_LIST: [&'static MessageVTable] = [..];

thread_local! {
    static MESSAGES: FxHashMap<(&'static str, &'static str), &'static MessageVTable> = {
        MESSAGE_LIST.iter()
            .map(|vtable| ((vtable.protocol, vtable.name), *vtable))
            .collect()
    };
}

#[cfg(feature = "network")]
fn lookup_vtable(protocol: &str, name: &str) -> Option<&'static MessageVTable> {
    // Extend lifetimes to static in order to get `(&'static str, &'static str)`.
    // SAFETY: this pair doesn't overlive the function.
    let (protocol, name) = unsafe {
        (
            std::mem::transmute::<_, &'static str>(protocol),
            std::mem::transmute::<_, &'static str>(name),
        )
    };

    MESSAGES.with(|messages| messages.get(&(protocol, name)).copied())
}

pub(crate) fn init() {
    MESSAGES.with(|_| ());
}
