use std::{
    alloc, fmt,
    ptr::{self, NonNull},
};

use fxhash::{FxHashMap, FxHashSet};
use linkme::distributed_slice;
use metrics::Label;
use once_cell::sync::Lazy;
use smallbox::smallbox;

use super::Message;
use crate::dumping;

#[cfg(feature = "network")]
use rmp_serde::{decode, encode};

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

    // Cannot be `const ANY` until rust#119618.
    pub(super) fn any() -> Self {
        Self::new(VTABLE_ANY)
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
    pub(crate) vtable: &'static MessageVTable,
    pub(crate) data: M,
}

impl<M> MessageRepr<M>
where
    M: Message,
{
    pub(crate) fn new(message: M) -> Self {
        debug_assert_ne!(M::_type_id(), MessageTypeId::any());

        Self {
            vtable: message._vtable(),
            data: message,
        }
    }
}

impl MessageRepr {
    pub(super) unsafe fn alloc(vtable: &'static MessageVTable) -> NonNull<Self> {
        let ptr = alloc::alloc(vtable.repr_layout);

        let Some(ptr) = NonNull::new(ptr) else {
            alloc::handle_alloc_error(vtable.repr_layout);
        };

        ptr.cast()
    }

    pub(super) unsafe fn dealloc(ptr: NonNull<Self>) {
        let ptr = ptr.as_ptr();
        let vtable = (*ptr).vtable;

        alloc::dealloc(ptr.cast(), vtable.repr_layout);
    }
}

// === MessageVTable ===

// Reexported in `elfo::_priv`.
/// Message Virtual Table.
pub struct MessageVTable {
    pub(super) repr_layout: alloc::Layout, // of `MessageRepr<M>`
    pub(super) name: &'static str,
    pub(super) protocol: &'static str,
    pub(super) labels: [Label; 2],
    pub(super) dumping_allowed: bool, // TODO: introduce `DumpingMode`.
    // TODO: field ordering (better for cache)
    // TODO:
    // pub(super) deserialize_any: fn(&mut dyn erased_serde::Deserializer<'_>) ->
    // Result<AnyMessage, erased_serde::Error>,
    #[cfg(feature = "network")]
    pub(super) read_msgpack: unsafe fn(&[u8], NonNull<MessageRepr>) -> Result<(), decode::Error>,
    #[cfg(feature = "network")]
    #[allow(clippy::type_complexity)]
    pub(super) write_msgpack:
        unsafe fn(NonNull<MessageRepr>, &mut Vec<u8>, usize) -> Result<(), encode::Error>,
    pub(super) debug: unsafe fn(NonNull<MessageRepr>, &mut fmt::Formatter<'_>) -> fmt::Result,
    pub(super) clone: unsafe fn(NonNull<MessageRepr>, NonNull<MessageRepr>),
    pub(super) erase: unsafe fn(NonNull<MessageRepr>) -> dumping::ErasedMessage,
    pub(super) deserialize_any: unsafe fn(
        deserializer: &mut dyn erased_serde::Deserializer<'_>,
        out_ptr: NonNull<MessageRepr>,
    ) -> Result<(), erased_serde::Error>,
    pub(super) drop: unsafe fn(NonNull<MessageRepr>),
}

static VTABLE_ANY: &MessageVTable = &MessageVTable {
    repr_layout: alloc::Layout::new::<()>(),
    name: "",
    protocol: "",
    labels: [
        Label::from_static_parts("", ""),
        Label::from_static_parts("", ""),
    ],
    dumping_allowed: false,
    #[cfg(feature = "network")]
    read_msgpack: |_, _| unreachable!(),
    #[cfg(feature = "network")]
    write_msgpack: |_, _, _| unreachable!(),
    debug: |_, _| unreachable!(),
    clone: |_, _| unreachable!(),
    erase: |_| unreachable!(),
    deserialize_any: |_, _| unreachable!(),
    drop: |_| unreachable!(),
};

impl MessageVTable {
    // Reexported in `elfo::_priv`.
    #[doc(hidden)]
    pub const fn new<M: Message>(
        name: &'static str,
        protocol: &'static str,
        dumping_allowed: bool,
    ) -> Self {
        Self {
            repr_layout: alloc::Layout::new::<MessageRepr<M>>(),
            name,
            protocol,
            labels: [
                Label::from_static_parts("message", name),
                Label::from_static_parts("protocol", protocol),
            ],
            dumping_allowed,
            debug: vtablefns::debug::<M>,
            clone: vtablefns::clone::<M>,
            erase: vtablefns::erase::<M>,
            deserialize_any: vtablefns::deserialize_any::<M>,
            drop: vtablefns::drop::<M>,
            #[cfg(feature = "network")]
            read_msgpack: vtablefns::read_msgpack::<M>,
            #[cfg(feature = "network")]
            write_msgpack: vtablefns::write_msgpack::<M>,
        }
    }
}

mod vtablefns {
    use super::*;

    pub(super) unsafe fn drop<M>(ptr: NonNull<MessageRepr>) {
        ptr::drop_in_place(ptr.cast::<MessageRepr<M>>().as_ptr());
    }

    pub(super) unsafe fn clone<M: Clone>(ptr: NonNull<MessageRepr>, out_ptr: NonNull<MessageRepr>) {
        ptr::write(
            out_ptr.cast::<MessageRepr<M>>().as_ptr(),
            ptr.cast::<MessageRepr<M>>().as_ref().clone(),
        );
    }

    pub(super) unsafe fn debug<M: fmt::Debug>(
        ptr: NonNull<MessageRepr>,
        f: &mut fmt::Formatter<'_>,
    ) -> fmt::Result {
        let data = &ptr.cast::<MessageRepr<M>>().as_ref().data;
        fmt::Debug::fmt(data, f)
    }

    pub(super) unsafe fn erase<M: Message>(ptr: NonNull<MessageRepr>) -> dumping::ErasedMessage {
        let data = ptr.cast::<MessageRepr<M>>().as_ref().data.clone();
        smallbox!(data)
    }

    pub(super) unsafe fn deserialize_any<M: Message>(
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
        pub(super) unsafe fn read_msgpack<M: Message>(
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

        pub(super) unsafe fn write_msgpack<M: Message>(
            ptr: NonNull<MessageRepr>,
            out: &mut Vec<u8>,
            limit: usize,
        ) -> Result<(), encode::Error> {
            let data = &ptr.cast::<MessageRepr<M>>().as_ref().data;
            let mut out = LimitedWrite(out, limit);
            encode::write_named(&mut out, data)
        }
    });
}

// === VTable registration & lookup ===

// Reexported in `elfo::_priv`.
#[doc(hidden)]
#[distributed_slice]
pub static MESSAGE_VTABLES_LIST: [&'static MessageVTable] = [..];

static MESSAGE_VTABLES_MAP: Lazy<FxHashMap<(&'static str, &'static str), &'static MessageVTable>> =
    Lazy::new(|| {
        MESSAGE_VTABLES_LIST
            .iter()
            .map(|vtable| ((vtable.protocol, vtable.name), *vtable))
            .collect()
    });

impl MessageVTable {
    pub(crate) fn lookup(protocol: &str, name: &str) -> Option<&'static Self> {
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
}

pub(crate) fn check_uniqueness() -> Result<(), Vec<(String, String)>> {
    if MESSAGE_VTABLES_MAP.len() == MESSAGE_VTABLES_LIST.len() {
        return Ok(());
    }

    Err(MESSAGE_VTABLES_LIST
        .iter()
        .filter(|vtable| {
            let stored = MessageVTable::lookup(vtable.protocol, vtable.name).unwrap();
            MessageTypeId::new(stored) != MessageTypeId::new(vtable)
        })
        .map(|vtable| (vtable.protocol.to_string(), vtable.name.to_string()))
        .collect::<FxHashSet<_>>()
        .into_iter()
        .collect::<Vec<_>>())
}

// === LimitedWrite ===

cfg_network!({
    use std::io;

    // The compiler requires all arguments to be visible.
    struct LimitedWrite<W>(W, usize);

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
