//! TODO

#![warn(rust_2018_idioms, unreachable_pub, missing_docs)]

mod actor;
mod codec;
mod config;
mod discovery;
mod listener;
mod node_map;
mod protocol;

pub use actor::new;
