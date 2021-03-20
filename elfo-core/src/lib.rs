#![warn(rust_2018_idioms, unreachable_pub)]

#[macro_use]
extern crate static_assertions;

// TODO: missing_docs

pub use crate::{
    addr::Addr,
    context::Context,
    envelope::Envelope,
    group::{ActorGroup, Schema},
    message::{Message, Request},
    request_table::ResponseToken,
    topology::Topology,
};

/// Returns the contents of a `Option<T>`'s `Some(T)`, otherwise it returns
/// early from the function. Can alternatively have an `else` branch, or an
/// alternative "early return" statement, like `break` or `continue` for loops.
macro_rules! ward {
    ($o:expr) => (ward!($o, else { return; }));
    ($o:expr, else $body:block) => { if let Some(x) = $o { x } else { $body }; };
    ($o:expr, $early:stmt) => (ward!($o, else { $early }));
}

pub mod errors;
pub mod routers;
pub mod trace_id;

mod addr;
mod address_book;
mod context;
mod demux;
mod envelope;
mod exec;
mod group;
mod macros;
mod mailbox;
mod message;
mod object;
mod request_table;
mod supervisor;
mod topology;

#[doc(hidden)]
pub mod _priv {
    pub use crate::{
        envelope::{AnyMessageBorrowed, AnyMessageOwned, EnvelopeBorrowed, EnvelopeOwned},
        message::{AnyMessage, LocalTypeId, MessageVTable, MESSAGE_LIST},
    };
    pub use linkme;
    pub use serde;
    pub use smallbox;
    pub use static_assertions::assert_impl_all;
}
