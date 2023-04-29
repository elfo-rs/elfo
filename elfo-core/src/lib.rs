#![warn(rust_2018_idioms, unreachable_pub)] // TODO: add `missing_docs`.
#![cfg_attr(docsrs, feature(doc_cfg))]

#[macro_use]
extern crate static_assertions;
#[macro_use]
extern crate elfo_utils;

// To make `#[message]` and `msg!` work inside `elfo-core`.
extern crate self as elfo_core;

// TODO: revise this list.
pub use crate::{
    actor::{ActorMeta, ActorStatus, ActorStatusKind},
    addr::Addr,
    config::Config,
    context::{Context, RequestBuilder},
    envelope::Envelope,
    group::{ActorGroup, Schema},
    init::{start, try_start},
    local::{Local, MoveOwnership},
    message::{Message, Request},
    request_table::ResponseToken,
    source::SourceHandle,
    topology::Topology,
};
pub use elfo_macros::{message_core as message, msg_core as msg};

pub mod config;
pub mod dumping;
pub mod errors;
pub mod group;
pub mod logging;
pub mod messages;
pub mod node;
pub mod routers;
pub mod scope;
pub mod signal;
pub mod stream;
#[cfg(feature = "unstable-stuck-detection")]
pub mod stuck_detection;
pub mod time;
#[deprecated(note = "use `elfo::scope` instead")]
pub mod tls;
pub mod topology;
pub mod tracing;

#[deprecated(note = "use `elfo::tracing` instead")]
pub mod trace_id {
    pub use crate::tracing::TraceId;

    pub fn generate() -> TraceId {
        TraceId::generate()
    }
}

mod actor;
mod addr;
mod address_book;
mod context;
mod demux;
mod envelope;
mod exec;
mod init;
mod local;
mod macros;
mod mailbox;
mod memory_tracker;
mod message;
mod object;
mod permissions;
mod request_table;
mod runtime;
mod source;
mod subscription;
mod supervisor;
mod telemetry;
mod thread;

#[doc(hidden)]
pub mod _priv {
    pub mod node {
        pub fn set_node_no(node_no: crate::node::NodeNo) {
            crate::node::set_node_no(node_no)
        }
    }

    pub use crate::{
        envelope::{AnyMessageBorrowed, AnyMessageOwned, EnvelopeBorrowed, EnvelopeOwned},
        init::do_start,
        message::{AnyMessage, MessageVTable, MESSAGE_LIST},
        permissions::{AtomicPermissions, Permissions},
    };
    pub use linkme;
    pub use metrics;
    pub use serde;
    pub use smallbox;
    pub use static_assertions::assert_impl_all;
}
