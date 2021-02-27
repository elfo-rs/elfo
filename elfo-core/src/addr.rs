use std::num::NonZeroU64;

// TODO: improve `Debug` and `Display` instances.

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Addr(NonZeroU64);
