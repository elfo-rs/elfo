use std::ops::Deref;

use derive_more::From;
use serde::{
    de::{self, Deserializer},
    ser::Serializer,
    Deserialize, Serialize,
};

/// Messages must be instances of `Serialize` and `Deserialize` because of
/// network communication and message dumping. However, in some cases, it's
/// desired to have messages that cannot be serialized. For instance, when
/// sharding TCP sockets.
///
/// `Local<T>` implements `Serialize` and `Deserialize` for any `T`. Meanwhile,
/// it can be serialized (but w/o useful information), it cannot be deserialized
/// (it returns an error on attempts).
#[derive(Debug, Clone, Copy, Eq, PartialEq, Default, From)]
pub struct Local<T>(pub T);

impl<T> Local<T> {
    #[inline]
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T> Deref for Local<T> {
    type Target = T;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T> Serialize for Local<T> {
    #[inline]
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // TODO: fail to serialize in the network context.
        // TODO: practically `T` is `Debug`, use it?
        serializer.serialize_unit_struct("Local")
    }
}

impl<'de, T> Deserialize<'de> for Local<T> {
    fn deserialize<D>(_deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Err(de::Error::custom("Local<T> cannot be deserialized"))
    }
}
