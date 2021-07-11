#![warn(rust_2018_idioms, unreachable_pub)] // TODO: add `missing_docs`.
#![cfg_attr(docsrs, feature(doc_cfg))]

#[macro_use]
extern crate static_assertions;
#[macro_use]
extern crate elfo_utils;

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

pub mod config;
pub mod errors;
pub mod messages;
pub mod routers;
pub mod signal;
pub mod stream;
pub mod time;
pub mod tls;
pub mod topology;
pub mod trace_id;

mod actor;
mod addr;
mod address_book;
mod context;
mod demux;
mod dumping;
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
    pub mod dumping {
        pub use crate::dumping::*;

        #[inline]
        pub fn of<C, K, S>(context: &crate::Context<C, K, S>) -> &Dumper {
            context.dumper()
        }
    }

    pub use crate::{
        envelope::{AnyMessageBorrowed, AnyMessageOwned, EnvelopeBorrowed, EnvelopeOwned},
        message::{AnyMessage, LocalTypeId, MessageVTable, MESSAGE_LIST},
        object::ObjectMeta,
        start::do_start,
    };
    pub use linkme;
    pub use serde;
    pub use smallbox;
    pub use static_assertions::assert_impl_all;
}
