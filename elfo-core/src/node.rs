use std::sync::atomic::{AtomicU16, Ordering};

#[cfg(feature = "unstable")]
pub use crate::addr::NodeNo;

static NODE_NO: AtomicU16 = AtomicU16::new(0);

/// Returns the current `node_no`.
pub fn node_no() -> Option<crate::addr::NodeNo> {
    crate::addr::NodeNo::from_bits(NODE_NO.load(Ordering::Relaxed))
}

/// Sets the current `node_no`.
/// The value `65535` is used if isn't called.
///
/// `node_no` should be set during setup stage, now reexport via `_priv`.
pub(crate) fn set_node_no(node_no: u16) {
    NODE_NO.store(node_no, Ordering::Relaxed)
}
