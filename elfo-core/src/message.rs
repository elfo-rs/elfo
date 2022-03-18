use std::{any::Any, fmt};

use fxhash::FxHashMap;
use linkme::distributed_slice;
use metrics::Label;
use serde::{Deserialize, Serialize};
use smallbox::{smallbox, SmallBox};

use crate::dumping;

pub type LocalTypeId = u32;

pub trait Message: fmt::Debug + Clone + Any + Send + Serialize + for<'de> Deserialize<'de> {
    #[doc(hidden)]
    const _LTID: LocalTypeId;

    /// A protocol's name.
    /// Usually, it's a crate name where the message is defined.
    const PROTOCOL: &'static str;
    /// Just a message's name.
    const NAME: &'static str;

    #[doc(hidden)]
    const LABELS: &'static [Label];

    #[doc(hidden)]
    const DUMPING_ALLOWED: bool; // TODO: introduce `DumpingMode`.

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
    ltid: LocalTypeId,
    data: SmallBox<dyn Any + Send, [u8; 184]>,
}

impl AnyMessage {
    #[inline]
    pub fn new<M: Message>(message: M) -> Self {
        message._touch();

        AnyMessage {
            ltid: M::_LTID,
            data: smallbox!(message),
        }
    }

    #[inline]
    pub fn name(&self) -> &'static str {
        with_vtable(self.ltid, |vtable| vtable.name)
    }

    #[inline]
    pub fn protocol(&self) -> &'static str {
        with_vtable(self.ltid, |vtable| vtable.protocol)
    }

    #[inline]
    #[doc(hidden)]
    pub fn labels(&self) -> &'static [Label] {
        with_vtable(self.ltid, |vtable| vtable.labels)
    }

    #[inline]
    #[doc(hidden)]
    pub fn dumping_allowed(&self) -> bool {
        with_vtable(self.ltid, |vtable| vtable.dumping_allowed)
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
        if M::_LTID != self.ltid {
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
        with_vtable(self.ltid, |vtable| (vtable.erase)(self))
    }
}

impl Clone for AnyMessage {
    #[inline]
    fn clone(&self) -> Self {
        with_vtable(self.ltid, |vtable| (vtable.clone)(self))
    }
}

impl fmt::Debug for AnyMessage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        with_vtable(self.ltid, |vtable| (vtable.debug)(self, f))
    }
}

// Message Virtual Table.

// Reexported in `elfo::_priv`.
#[derive(Clone)]
pub struct MessageVTable {
    pub ltid: LocalTypeId,
    pub name: &'static str,
    pub protocol: &'static str,
    pub labels: &'static [Label],
    pub dumping_allowed: bool,
    pub clone: fn(&AnyMessage) -> AnyMessage,
    pub debug: fn(&AnyMessage, &mut fmt::Formatter<'_>) -> fmt::Result,
    pub erase: fn(&AnyMessage) -> dumping::ErasedMessage,
}

#[distributed_slice]
pub static MESSAGE_LIST: [MessageVTable] = [..];

thread_local! {
    // TODO: access it speculatively during initialization.
    // TODO: use simd + `SmallVec<[Vec<MessageVTable>; N]>` and sequential LTIDs.
    static MESSAGE_BY_LTID: FxHashMap<LocalTypeId, MessageVTable> = {
        MESSAGE_LIST.iter()
            .map(|vtable| (vtable.ltid, vtable.clone()))
            .collect()
    };
}

fn with_vtable<R>(ltid: LocalTypeId, f: impl FnOnce(&MessageVTable) -> R) -> R {
    MESSAGE_BY_LTID.with(|map| f(map.get(&ltid).expect("invalid LTID")))
}

pub(crate) fn init() {
    MESSAGE_BY_LTID.with(|_| ());
}
