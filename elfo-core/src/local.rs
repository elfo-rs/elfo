use std::{fmt, ops::Deref, sync::Arc};

use derive_more::From;
use parking_lot::Mutex;
use serde::{
    de::{self, Deserializer},
    ser::Serializer,
    Deserialize, Serialize,
};

/// Used to send values that cannot be serialized.
/// Works inside the current node only.
///
/// Messages must be instances of `Clone`, `Serialize` and `Deserialize` because
/// of network communication and message dumping. However, in some cases, it's
/// desired to have messages that cannot satisfy requirements.
/// For instance, when sending channels.
///
/// `Local<T>` implements `Debug`, `Serialize` and `Deserialize` for any `T`.
/// Meanwhile, it can be serialized (but w/o useful information), it cannot be
/// deserialized (it returns an error on attempts).
///
/// # Example
/// ```ignore
/// #[message]
/// pub struct OpenDirectChannel {
///     pub tx: Local<mpsc::Sender>,
/// }
/// ```
#[derive(Clone, Copy, PartialEq, Eq, Default, From)]
pub struct Local<T>(T);

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

impl<T> fmt::Debug for Local<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "<local>")
    }
}

impl<T> Serialize for Local<T> {
    #[inline]
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // TODO: fail to serialize in the network context.
        serializer.serialize_str("<local>")
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

/// Used to transfer ownership over messaging.
///
/// Messages must be instances of `Clone`, `Serialize` and `Deserialize` because
/// of network communication and message dumping. However, in some cases, it's
/// desired to have messages that cannot satisfy requirements.
/// For instance, when sending sockets or files.
///
/// `MoveOwnership<T>` implements `Debug`, `Clone`, `Serialize` and
/// `Deserialize` for any `T`. Meanwhile, it can be serialized (but w/o useful
/// information), it cannot be deserialized (it returns an error on attempts).
///
/// # Example
/// ```ignore
/// #[message]
/// pub struct HandleFile {
///     pub path: PathBuf,
///     pub file: MoveOwnership<File>,
/// }
/// ```
pub struct MoveOwnership<T>(Arc<Mutex<Option<T>>>);

impl<T> MoveOwnership<T> {
    /// Takes the value. Next calls will return `None`.
    pub fn take(&self) -> Option<T> {
        self.0.lock().take()
    }
}

impl<T> From<T> for MoveOwnership<T> {
    fn from(value: T) -> Self {
        Self(Arc::new(Mutex::new(Some(value))))
    }
}

impl<T> Clone for MoveOwnership<T> {
    fn clone(&self) -> MoveOwnership<T> {
        Self(self.0.clone())
    }
}

impl<T> fmt::Debug for MoveOwnership<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "<local>")
    }
}

impl<T> Serialize for MoveOwnership<T> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // TODO: fail to serialize in the network context.
        serializer.serialize_str("<local>")
    }
}

impl<'de, T> Deserialize<'de> for MoveOwnership<T> {
    fn deserialize<D>(_deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Err(de::Error::custom("MoveOwnership<T> cannot be deserialized"))
    }
}
