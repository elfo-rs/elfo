use std::{
    alloc, fmt,
    ptr::{self, NonNull},
};

use metrics::Label;
use serde::{Deserialize, Serialize};
use smallbox::smallbox;

use crate::dumping;

pub use self::{any::*, protocol::*, repr::*};

mod any;
mod protocol;
mod repr;

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
