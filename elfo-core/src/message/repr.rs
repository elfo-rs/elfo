use std::{
    alloc, fmt,
    ptr::{self, NonNull},
};

use fxhash::{FxHashMap, FxHashSet};
use metrics::Label;
use once_cell::sync::Lazy;
use smallbox::smallbox;

use super::Message;
use crate::dumping;

#[cfg(feature = "network")]
use rmp_serde::{decode, encode};

// === MessageTypeId ===

/// A unique (inside a compilation target) identifier of a message type.
// Internally, it's simply an address of corresponding vtable.
// ~
// However, we cannot cast it into integer in the const context,
// so we're forced to use a raw pointer and `ptr::eq()`.
//
// `NULL` is used for `AnyMessage`.
//
// Reexported in `elfo::_priv`.
#[doc(hidden)]
#[derive(Clone, Copy, Debug)]
pub struct MessageTypeId(*const ());

/// SAFETY: used only for comparison, safe to send across threads.
unsafe impl Send for MessageTypeId {}
/// SAFETY: used only for comparison, safe to sync across threads.
unsafe impl Sync for MessageTypeId {}

impl MessageTypeId {
    #[inline]
    pub const fn new(vtable: &'static MessageVTable) -> Self {
        Self(vtable as *const _ as *const ())
    }

    pub(super) const fn any() -> Self {
        Self(ptr::null())
    }
}

impl PartialEq for MessageTypeId {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        ptr::eq(self.0, other.0)
    }
}

// === MessageRepr ===

/// A message representation as a cpp-style object.
///
/// Initially, it's created from a typed message as [`MessageRepr<M>`], then
/// * for [`AnyMessage`]: moved directly to heap
/// * for [`Envelope`]: becomes a part and whole envelope is moved to heap
///
/// All subsequent accesses are done via `NonNull<MessageRepr>` and require
/// using the virtual table ([`MessageVTable`]) for all operations.
///
/// [`AnyMessage`]: crate::AnyMessage
/// [`Envelope`]: crate::Envelope
// `vtable` must be first
#[repr(C)]
// It's `pub` only because used in private methods of `AnyMessage`.
// Actually, it's not reexported at all.
#[doc(hidden)]
pub struct MessageRepr<M = ()> {
    pub(crate) vtable: &'static MessageVTable,
    pub(crate) data: M,
}

impl<M: Message> MessageRepr<M> {
    /// Creates a new typed `MessageRepr` on stack.
    /// Cannot be created for `AnyMessage` (which also implements `Message`).
    pub(crate) fn new(message: M) -> Self {
        debug_assert_ne!(M::_type_id(), MessageTypeId::any());

        Self {
            vtable: message._vtable(),
            data: message,
        }
    }
}

// Protection against footgun.
assert_not_impl_any!(MessageRepr: Clone);

impl<M: Message> Clone for MessageRepr<M> {
    fn clone(&self) -> Self {
        Self {
            vtable: self.vtable,
            data: self.data.clone(),
        }
    }
}

// === MessageVTable ===

/// Message Virtual Table.
// ~
// TODO: this struct is big enough and takes several cache lines.
//       add `repr(C)` and reorder by frequency of access for better locality.
// Reexported in `elfo::_priv`.
#[doc(hidden)]
#[non_exhaustive] // must be created only via `MessageVTable::new()`
pub struct MessageVTable {
    pub(super) repr_layout: alloc::Layout, // of `MessageRepr<M>`
    pub(super) name: &'static str,
    pub(super) protocol: &'static str,
    pub(super) labels: [Label; 2],    // protocol + name for `metrics`
    pub(super) dumping_allowed: bool, // TODO: introduce `DumpingMode`.
    #[cfg(feature = "network")]
    pub(super) read_msgpack: unsafe fn(&[u8], NonNull<MessageRepr>) -> Result<(), decode::Error>,
    #[cfg(feature = "network")]
    #[allow(clippy::type_complexity)]
    pub(super) write_msgpack:
        unsafe fn(NonNull<MessageRepr>, &mut Vec<u8>, usize) -> Result<(), encode::Error>,
    pub(super) debug: unsafe fn(NonNull<MessageRepr>, &mut fmt::Formatter<'_>) -> fmt::Result,
    pub(super) clone: unsafe fn(NonNull<MessageRepr>, NonNull<MessageRepr>),
    // TODO: remove and use `as_serialize_any` in the dumper after benchmarking.
    pub(super) erase: unsafe fn(NonNull<MessageRepr>) -> dumping::ErasedMessage,
    pub(super) as_serialize_any:
        unsafe fn(NonNull<MessageRepr>) -> NonNull<dyn erased_serde::Serialize>,
    pub(super) deserialize_any: unsafe fn(
        &mut dyn erased_serde::Deserializer<'_>,
        NonNull<MessageRepr>,
    ) -> Result<(), erased_serde::Error>,
    pub(super) drop: unsafe fn(NonNull<MessageRepr>),
}

impl MessageVTable {
    /// Creates a new vtable for the provided message type.
    /// This is the only way to create a vtable.
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
            as_serialize_any: vtablefns::as_serialize_any::<M>,
            deserialize_any: vtablefns::deserialize_any::<M>,
            drop: vtablefns::drop::<M>,
            #[cfg(feature = "network")]
            read_msgpack: vtablefns::read_msgpack::<M>,
            #[cfg(feature = "network")]
            write_msgpack: vtablefns::write_msgpack::<M>,
        }
    }
}

/// Generic vtable's functions for monomorphization in [`MessageVTable::new()`].
///
/// All functions are `unsafe` because they work with raw pointers.
///
/// # Safety
///
/// Common safety requirements for all functions:
/// * input pointers (`ptr`) must be [valid] for reading `MessageRepr<M>`.
/// * output pointers (`out_ptr`) must be [valid] for writing `MessageRepr<M>`.
///
/// [valid]: https://doc.rust-lang.org/stable/std/ptr/index.html#safety
mod vtablefns {
    use super::*;

    /// # Safety
    ///
    /// Data behind `ptr` cannot be accessed after this call.
    /// Note that vtable is still can be accessed.
    pub(super) unsafe fn drop<M>(ptr: NonNull<MessageRepr>) {
        ptr::drop_in_place(ptr.cast::<MessageRepr<M>>().as_ptr());
    }

    pub(super) unsafe fn clone<M: Message>(
        ptr: NonNull<MessageRepr>,
        out_ptr: NonNull<MessageRepr>,
    ) {
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

    /// # Safety
    ///
    /// The result pointer is valid only during the lifetime of `ptr`.
    pub(super) unsafe fn as_serialize_any<M: Message>(
        ptr: NonNull<MessageRepr>,
    ) -> NonNull<dyn erased_serde::Serialize> {
        let data = &ptr.cast::<MessageRepr<M>>().as_ref().data;
        let ser = data as &dyn erased_serde::Serialize;
        NonNull::new_unchecked(ser as *const _ as *mut _)
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

/// A list of all registered message vtables via the `linkme` crate.
/// Used only for collecting, all lookups are done via a hashmap.
// Reexported in `elfo::_priv`.
#[doc(hidden)]
#[linkme::distributed_slice]
pub static MESSAGE_VTABLES_LIST: [&'static MessageVTable] = [..];

static MESSAGE_VTABLES_MAP: Lazy<FxHashMap<(&'static str, &'static str), &'static MessageVTable>> =
    Lazy::new(|| {
        MESSAGE_VTABLES_LIST
            .iter()
            .map(|vtable| ((vtable.protocol, vtable.name), *vtable))
            .collect()
    });

impl MessageVTable {
    /// Finds a vtable by protocol and name.
    /// Used for deserialization of `AnyMessage` and in networking.
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

/// Checks that all registered message have different protocol and name.
/// Returns a list of duplicates if it's violated.
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
