use std::borrow::Borrow;

use fxhash::{FxHashMap, FxHashSet};

use super::{MessageTypeId, MessageVTable};

/// A list of all registered message vtables via the `linkme` crate.
/// Used only for collecting, all lookups are done via a hashmap.
// Reexported in `elfo::_priv`.
#[doc(hidden)]
#[linkme::distributed_slice]
pub static MESSAGE_VTABLES_LIST: [&'static MessageVTable] = [..];

static MESSAGE_VTABLES_MAP: vtables_map::VTablesMap = vtables_map::VTablesMap::new();

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

#[derive(PartialEq, Eq, Hash)]
struct Signature([&'static str; 2]); // [protocol, name]

impl<'a> Borrow<[&'a str; 2]> for Signature {
    fn borrow(&self) -> &[&'a str; 2] {
        &self.0
    }
}

impl MessageVTable {
    /// Finds a vtable by protocol and name.
    /// Used for deserialization of `AnyMessage` and in networking.
    pub(crate) fn lookup(protocol: &str, name: &str) -> Option<&'static Self> {
        MESSAGE_VTABLES_MAP.get(protocol, name)
    }

    #[cfg(miri)]
    pub(crate) fn register_for_miri(&'static self) {
        MESSAGE_VTABLES_MAP.register(self);
    }
}

#[cfg(not(miri))]
mod vtables_map {
    use std::sync::LazyLock;

    use super::*;

    pub(super) struct VTablesMap(LazyLock<FxHashMap<Signature, &'static MessageVTable>>);

    impl VTablesMap {
        pub(super) const fn new() -> Self {
            let inner: LazyLock<_> = LazyLock::new(|| {
                MESSAGE_VTABLES_LIST
                    .iter()
                    .map(|vtable| (Signature([vtable.protocol, vtable.name]), *vtable))
                    .collect()
            });

            Self(inner)
        }

        pub(super) fn get(&self, protocol: &str, name: &str) -> Option<&'static MessageVTable> {
            self.0.get(&[protocol, name]).copied()
        }

        pub(super) fn len(&self) -> usize {
            self.0.len()
        }
    }
}

#[cfg(miri)]
mod vtables_map {
    use std::sync::Mutex;

    use super::*;

    // parking-lot doesn't compile with `-Zmiri-strict-provenance`,
    // so we cannot use `parking_lot::Mutex` and `Lazy` here.
    pub(super) struct VTablesMap(Mutex<Option<FxHashMap<Signature, &'static MessageVTable>>>);

    impl VTablesMap {
        pub(super) const fn new() -> Self {
            Self(Mutex::new(None))
        }

        pub(super) fn get(&self, protocol: &str, name: &str) -> Option<&'static MessageVTable> {
            let guard = self.0.lock().unwrap();
            guard.as_ref()?.get(&[protocol, name]).copied()
        }

        pub(super) fn len(&self) -> usize {
            self.0.lock().unwrap().as_ref().map_or(0, |m| m.len())
        }

        pub(super) fn register(&self, vtable: &'static MessageVTable) {
            let key = Signature([vtable.protocol, vtable.name]);
            let mut map = self.0.lock().unwrap();
            map.get_or_insert_with(<_>::default).insert(key, vtable);
        }
    }
}
