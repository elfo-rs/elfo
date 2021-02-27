use std::num::NonZeroU64;

use derive_more::{Display, From, Into};
use serde::{Deserialize, Serialize};

// TODO: improve `Debug` and `Display` instances.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[derive(Serialize, Deserialize, Into, From, Display)]
#[display(fmt = "{}", _0)]
pub struct TraceId(NonZeroU64);

impl TraceId {
    #[inline]
    pub fn new(value: u64) -> Option<Self> {
        NonZeroU64::new(value).map(TraceId)
    }
}
