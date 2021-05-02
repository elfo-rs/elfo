#![warn(rust_2018_idioms, unreachable_pub)]
// TODO: add `missing_docs`.

#[macro_use]
extern crate static_assertions;

pub use crate::{
    actor::ActorStatus,
    addr::Addr,
    config::Config,
    context::{Context, RequestBuilder},
    envelope::Envelope,
    group::{ActorGroup, Schema},
    local::Local,
    message::{Message, Request},
    request_table::ResponseToken,
    start::{start, try_start},
    topology::Topology,
};

#[macro_use]
mod utils;

pub mod config;
pub mod errors;
pub mod messages;
pub mod routers;
pub mod time;
pub mod topology;
pub mod trace_id;

mod actor;
mod addr;
mod address_book;
mod context;
mod demux;
mod envelope;
mod exec;
mod group;
mod local;
mod macros;
mod mailbox;
mod message;
mod object;
mod request_table;
mod start;
mod supervisor;

#[doc(hidden)]
pub mod _priv {
    pub use crate::{
        envelope::{AnyMessageBorrowed, AnyMessageOwned, EnvelopeBorrowed, EnvelopeOwned},
        message::{AnyMessage, LocalTypeId, MessageVTable, MESSAGE_LIST},
        start::do_start,
    };
    pub use linkme;
    pub use serde;
    pub use smallbox;
    pub use static_assertions::assert_impl_all;
}
