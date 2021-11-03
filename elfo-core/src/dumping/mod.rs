#[allow(unreachable_pub)] // Actually, it's reachable via `elfo::_priv`.
pub use self::{
    dump_item::{Direction, DumpItem, ErasedMessage, MessageKind, Timestamp},
    dumper::Dumper,
    hider::hide,
    sequence_no::SequenceNo,
};

use serde::Deserialize;

mod dump_item;
mod dumper;
mod hider;
mod sequence_no;

#[derive(Deserialize)]
#[serde(default)]
pub(crate) struct DumpingConfig {
    disabled: bool,
    max_rate: u64,
}

impl Default for DumpingConfig {
    fn default() -> Self {
        Self {
            disabled: false,
            max_rate: 100_000,
        }
    }
}

#[doc(hidden)]
pub mod _priv {
    pub use super::*;

    #[inline]
    pub fn of<C: 'static, K, S>(context: &crate::Context<C, K, S>) -> &Dumper {
        context.dumper()
    }

    pub fn set_in_dumping(flag: bool) {
        hider::set_in_dumping(flag);
    }
}
