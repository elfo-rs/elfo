#![warn(rust_2018_idioms, unreachable_pub)]

pub use elfo_core::*;
pub use elfo_macros::{message, msg};

pub mod prelude {
    pub use super::{assert_msg, assert_msg_eq, message, msg, ActorGroup, Context, Schema};
}

pub mod actors {
    pub use elfo_core::configurer;
}

#[cfg(feature = "test-util")]
pub use elfo_test as test;
