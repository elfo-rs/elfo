#![warn(rust_2018_idioms, unreachable_pub)] // TODO: add `missing_docs`.
#![cfg_attr(docsrs, feature(doc_cfg))]

#[macro_use]
extern crate static_assertions;
#[macro_use]
extern crate elfo_utils;

// To make `#[message]` and `msg!` work inside `elfo-core`.
extern crate self as elfo_core;

// TODO: revise this list, what about `NodeNo`?
pub use crate::{
    actor::{ActorMeta, ActorStatus, ActorStatusKind},
    address_book::{Addr, GroupNo},
    config::Config,
    context::{Context, RequestBuilder},
    envelope::Envelope,
    group::{ActorGroup, Blueprint, RestartPolicy, TerminationPolicy},
    local::{Local, MoveOwnership},
    message::{Message, Request},
    request_table::ResponseToken,
    source::{SourceHandle, UnattachedSource},
    topology::Topology,
};
pub use elfo_macros::{message_core as message, msg_core as msg};

#[macro_use]
mod macros;

pub mod config;
pub mod dumping;
pub mod errors;
pub mod init;
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
pub mod topology;
pub mod tracing;

mod actor;
mod address_book;
mod context;
mod demux;
mod envelope;
mod exec;
mod group;
mod local;
mod mailbox;
mod memory_tracker;
mod message;
mod object;
mod permissions;
#[cfg(all(feature = "network", feature = "unstable"))]
pub mod remote;
#[cfg(all(feature = "network", not(feature = "unstable")))]
mod remote;
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
        address_book::AddressBook,
        envelope::{
            AnyMessageBorrowed, AnyMessageOwned, EnvelopeBorrowed, EnvelopeOwned, MessageKind,
        },
        init::do_start,
        message::{AnyMessage, MessageVTable, MESSAGE_LIST},
        object::{GroupVisitor, Object, ObjectArc},
        permissions::{AtomicPermissions, Permissions},
        request_table::RequestId,
    };
    pub use linkme;
    pub use metrics;
    #[cfg(feature = "network")]
    pub use rmp_serde as rmps;
    pub use serde;
    pub use smallbox;
    pub use static_assertions::assert_impl_all;
}
