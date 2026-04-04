//! Utilities for testing of elfo actors.
//!
//! When testing actors, we are rarely interested in their internal state
//! directly. What matters is observable behavior: given a sequence of incoming
//! messages, what messages does the actor produce? This crate provides a
//! [`Proxy`] handle that lets a test drive an actor group in isolation, send
//! messages to it, and assert on its output.
//!
//! See the [Functional Testing] chapter of The Actoromicon for a detailed
//! guide with examples.
//!
//! [Functional Testing]: https://actoromicon.rs/ch06-01-functional-testing.html

#![cfg_attr(docsrs, feature(doc_cfg))]

pub use proxy::{Proxy, proxy};
pub use utils::{extract_message, extract_request};

#[cfg(feature = "unstable")]
pub use proxy::proxy_with_route;

mod proxy;
mod utils;
