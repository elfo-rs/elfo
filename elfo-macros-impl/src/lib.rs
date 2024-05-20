//! An internal crate for the `message` and `msg` macros.
//! Prefer the `elfo-macros` crate if you don't need to wrap macros.

extern crate proc_macro;

mod errors;
mod message;
mod msg;

pub use message::message_impl;
pub use msg::msg_impl;
