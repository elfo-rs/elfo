#![warn(rust_2018_idioms, unreachable_pub)]
// TODO: add `missing_docs`.

#[macro_use]
extern crate static_assertions;

pub use crate::{
    addr::Addr,
    context::{Context, RequestBuilder},
    envelope::Envelope,
    group::{ActorGroup, Schema},
    local::Local,
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
pub mod messages;
pub mod routers;
pub mod trace_id;

mod addr;
mod address_book;
mod config;
mod configurer;
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
mod supervisor;
mod topology;
mod utils;

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

pub mod actors {
    pub use crate::configurer::configurers;
}

// TODO: should it return `Result` instead of panicking?
pub async fn start(topology: Topology) {
    let entry = topology.book.vacant_entry();
    let addr = entry.addr();
    entry.insert(object::Object::new_actor(addr));

    let root = Context::new(topology.book.clone(), demux::Demux::default()).with_addr(addr);

    // XXX
    if let Some(group) = topology
        .actor_groups()
        .find(|group| group.name.contains("configurer"))
    {
        let config = Default::default();
        root.request(messages::UpdateConfig { config })
            .from(group.addr)
            .resolve()
            .await
            .expect("initial message cannot be delivered")
            .expect("entrypoint failed");
    }

    // TODO: handle SIGTERM and SIGINT.
    let () = futures::future::pending().await;
}
