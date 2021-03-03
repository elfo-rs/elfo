#![warn(rust_2018_idioms, unreachable_pub)]

// TODO: missing_docs

pub use crate::{
    context::Context,
    envelope::{Envelope, Message, ReplyToken, Request},
};

/// Returns the contents of a `Option<T>`'s `Some(T)`, otherwise it returns
/// early from the function. Can alternatively have an `else` branch, or an
/// alternative "early return" statement, like `break` or `continue` for loops.
macro_rules! ward {
    ($o:expr) => (ward!($o, else { return; }));
    ($o:expr, else $body:block) => { if let Some(x) = $o { x } else { $body }; };
    ($o:expr, $early:stmt) => (ward!($o, else { $early }));
}

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
    pub use static_assertions;
}
