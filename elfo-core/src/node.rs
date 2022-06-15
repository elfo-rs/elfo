use std::sync::atomic::{AtomicU16, Ordering};

pub(crate) const LOCAL_NODE_NO: u16 = 0;

static NODE_NO: AtomicU16 = AtomicU16::new(65535);

// TODO(v0.2): replace with `NonZeroU16`, revise the default one.
pub type NodeNo = u16;

/// Returns the current `node_no`.
pub fn node_no() -> NodeNo {
    NODE_NO.load(Ordering::Relaxed)
}

/// Sets the current `node_no`.
/// The value `65535` is used if isn't called.
///
/// `node_no` should be set during setup stage, now reexport via `_priv`.
pub(crate) fn set_node_no(node_no: NodeNo) {
    NODE_NO.store(node_no, Ordering::Relaxed)
}
