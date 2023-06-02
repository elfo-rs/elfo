use std::{any::Any, fmt};

use fxhash::FxHashMap;
use linkme::distributed_slice;
use metrics::Label;
use serde::{Deserialize, Serialize};
use smallbox::{smallbox, SmallBox};

use crate::dumping;

pub trait Message: fmt::Debug + Clone + Any + Send + Serialize + for<'de> Deserialize<'de> {
    #[doc(hidden)]
    const VTABLE: &'static MessageVTable;

    // Called while upcasting/downcasting to avoid
    // [rust#47384](https://github.com/rust-lang/rust/issues/47384).
    #[doc(hidden)]
    fn _touch(&self);
}

pub trait Request: Message {
    type Response: fmt::Debug + Clone + Send + Serialize;

    #[doc(hidden)]
    type Wrapper: Message + Into<Self::Response> + From<Self::Response>;
}

// Reexported in `elfo::_priv`.
pub struct AnyMessage {
    vtable: &'static MessageVTable,
    data: SmallBox<dyn Any + Send, [usize; 23]>,
}

impl AnyMessage {
    #[inline]
    pub fn new<M: Message>(message: M) -> Self {
        message._touch();

        AnyMessage {
            vtable: M::VTABLE,
            data: smallbox!(message),
        }
    }

    #[inline]
    pub fn name(&self) -> &'static str {
        self.vtable.name
    }

    #[inline]
    pub fn protocol(&self) -> &'static str {
        self.vtable.protocol
    }

    #[inline]
    pub fn labels(&self) -> &'static [Label] {
        self.vtable.labels
    }

    #[inline]
    pub fn dumping_allowed(&self) -> bool {
        self.vtable.dumping_allowed
    }

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

    #[inline]
    pub fn erase(&self) -> dumping::ErasedMessage {
        (self.vtable.erase)(self)
    }

    #[cfg(feature = "network")]
    #[inline]
    pub fn write_msgpack(&self, buffer: &mut [u8]) -> Result<usize, rmp_serde::encode::Error> {
        (self.vtable.write_msgpack)(self, buffer)
    }

    #[cfg(feature = "network")]
    #[inline]
    pub fn read_msgpack(
        protocol: &str,
        name: &str,
        buffer: &[u8],
    ) -> Result<Option<Self>, rmp_serde::decode::Error> {
        lookup_vtable(protocol, name)
            .map(|vtable| (vtable.read_msgpack)(buffer))
            .transpose()
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

// Message Virtual Table.

// Reexported in `elfo::_priv`.
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
    pub write_msgpack: fn(&AnyMessage, &mut [u8]) -> Result<usize, rmp_serde::encode::Error>,
    #[cfg(feature = "network")]
    pub read_msgpack: fn(&[u8]) -> Result<AnyMessage, rmp_serde::decode::Error>,
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
