use elfo_core::dumping::{Dump, Recorder};

use crate::dump_storage::DumpRegistry;

impl Recorder for DumpRegistry {
    fn record(&self, dump: Dump) {
        self.add(dump);
    }
}
