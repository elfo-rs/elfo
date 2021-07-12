use std::{any::Any, fmt};

use fxhash::FxHashMap;
use linkme::distributed_slice;
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
}

pub trait Request: Message {
    type Response: fmt::Debug + Clone + Send + Serialize;

    #[doc(hidden)]
    type Wrapper: Message + Into<Self::Response> + From<Self::Response>;
}

pub struct AnyMessage {
    ltid: LocalTypeId,
    data: SmallBox<dyn Any + Send, [u8; 64]>,
}

impl AnyMessage {
    #[inline]
    pub fn new<M: Message>(message: M) -> Self {
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
    pub fn is<T: 'static>(&self) -> bool {
        self.data.is::<T>()
    }

    #[inline]
    pub fn downcast_ref<T: 'static>(&self) -> Option<&T> {
        self.data.downcast_ref()
    }

    #[inline]
    pub fn downcast<M: Message>(self) -> Result<M, AnyMessage> {
        if M::_LTID != self.ltid {
            return Err(self);
        }

        Ok(self
            .data
            .downcast::<M>()
            .expect("cannot downcast")
            .into_inner())
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

#[derive(Clone)]
pub struct MessageVTable {
    pub ltid: LocalTypeId,
    pub name: &'static str,
    pub protocol: &'static str,
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
