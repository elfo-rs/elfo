//! Writes dumps of messages to files. [Configuration].
//!
//! Each line is a valid JSON. Lines can be unordered.
//!
//! For more details about dumping see [The Actoromicon].
//!
//! [Configuration]: crate::config::Config
//! [The Actoromicon]: https://actoromicon.rs/ch05-03-dumping.html

use std::sync::Arc;

use parking_lot::Mutex;
use tracing::error;

use elfo_core::{
    dumping::{self, Recorder},
    Blueprint,
};

use self::dump_storage::DumpStorage;

mod actor;
mod dump_storage;
mod file_registry;
mod recorder;
mod reporter;
mod rule_set;
mod serializer;

pub mod config;

/// Installs a global dump recorder and returns a group to handle dumps.
pub fn new() -> Blueprint {
    let storage = Arc::new(Mutex::new(DumpStorage::new()));
    let blueprint = actor::new(storage.clone());

    let is_ok = dumping::set_make_recorder(Box::new(move |class| {
        storage.lock().registry(class) as Arc<dyn Recorder>
    }));

    if !is_ok {
        error!("failed to set a dump recorder");
    }

    blueprint
}
