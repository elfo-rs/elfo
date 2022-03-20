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

pub struct AnyMessage {
    vtable: &'static MessageVTable,
    data: SmallBox<dyn Any + Send, [u8; 184]>,
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
    #[doc(hidden)]
    pub fn labels(&self) -> &'static [Label] {
        self.vtable.labels
    }

    #[inline]
    #[doc(hidden)]
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
    #[doc(hidden)]
    pub fn erase(&self) -> dumping::ErasedMessage {
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
}

#[distributed_slice]
pub static MESSAGE_LIST: [&'static MessageVTable] = [..];

thread_local! {
    static MESSAGE_BY_NAME: FxHashMap<(&'static str, &'static str), &'static MessageVTable> = {
        MESSAGE_LIST.iter()
            .map(|vtable| ((vtable.protocol, vtable.name), *vtable))
            .collect()
    };
}

pub(crate) fn init() {
    MESSAGE_BY_NAME.with(|_| ());
}
