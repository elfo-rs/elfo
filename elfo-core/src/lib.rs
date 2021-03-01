#![warn(rust_2018_idioms, unreachable_pub)]

// TODO: missing_docs

pub use crate::{
    context::Context,
    envelope::{Envelope, Message, ReplyToken},
};

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

#[doc(hidden)]
pub mod _priv {
    pub use crate::envelope::{
        AnyMessageBorrowed, AnyMessageOwned, EnvelopeBorrowed, EnvelopeOwned,
    };
}
