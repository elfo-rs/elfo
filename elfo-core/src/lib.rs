#![warn(rust_2018_idioms, unreachable_pub)]

// TODO: missing_docs
// TODO: add a prelude.

pub use crate::envelope::{Envelope, Message};

pub mod trace_id;

pub mod errors {
    pub use crate::mailbox::{SendError, TryRecvError, TrySendError};
}

mod addr;
mod address_book;
mod context;
mod envelope;
mod mailbox;
mod object;
