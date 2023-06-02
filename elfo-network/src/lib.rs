//! TODO

#![warn(rust_2018_idioms, unreachable_pub, missing_docs)]

#[macro_use]
extern crate elfo_utils;

mod actors;
mod codec;
mod config;
mod connection;
mod node_map;
mod protocol;

pub use actors::new;
