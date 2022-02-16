use std::{
    hash::{Hash, Hasher},
    thread,
};

pub(crate) type ThreadId = u64;

pub(crate) fn id() -> ThreadId {
    // TODO: just use `ThreadId::as_u64()` after stabilization.
    // See https://github.com/rust-lang/rust/issues/67939.
    struct RawIdExtractor(u64);

    impl Hasher for RawIdExtractor {
        fn write(&mut self, _bytes: &[u8]) {
            panic!("cannot extract thread ID");
        }

        fn write_u64(&mut self, i: u64) {
            self.0 = i;
        }

        fn finish(&self) -> u64 {
            self.0
        }
    }

    let opaque_id = thread::current().id();
    let mut extractor = RawIdExtractor(0);
    opaque_id.hash(&mut extractor);
    extractor.finish()
}
