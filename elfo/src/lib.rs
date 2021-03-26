#![warn(rust_2018_idioms, unreachable_pub)]

pub use elfo_core::*;
pub use elfo_macros::{message, msg};

pub mod prelude {
    pub use super::{message, msg, ActorGroup, Context, Schema};
}
