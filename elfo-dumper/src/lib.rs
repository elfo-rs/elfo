#![warn(rust_2018_idioms, unreachable_pub)]

use std::sync::Arc;

use parking_lot::Mutex;
use tracing::error;

use elfo_core::{
    dumping::{self, Recorder},
    Schema,
};

use self::storage::Storage;

mod actor;
mod config;
mod dump_buffer;
mod extract_name;
mod recorder;
mod storage;

/// Installs a global dump recorder and returns a group to handle dumps.
pub fn new() -> Schema {
    let storage = Arc::new(Mutex::new(Storage::new()));
    let schema = actor::new(storage.clone());

    let is_ok = dumping::set_make_recorder(Box::new(move |class| {
        storage.lock().registry(class) as Arc<dyn Recorder>
    }));

    if !is_ok {
        error!("failed to set a dump recorder");
    }

    schema
}
