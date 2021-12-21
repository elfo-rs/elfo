//! Includes `TraceId` and useful utilities around it.
//! For more details see [The Actoromicon](https://actoromicon.rs/ch05-04-tracing.html).

use std::cell::RefCell;

use self::generator::{ChunkRegistry, Generator};

pub use self::{trace_id::TraceId, validator::TraceIdValidator};

impl TraceId {
    /// Generates a new trace id according to [the schema](https://actoromicon.rs/ch05-04-tracing.html#traceid).
    pub fn generate() -> Self {
        GENERATOR.with(|cell| cell.borrow_mut().generate(&CHUNK_REGISTRY))
    }
}

static CHUNK_REGISTRY: ChunkRegistry = ChunkRegistry::new(0);
thread_local! {
    static GENERATOR: RefCell<Generator> = RefCell::new(Generator::default());
}

mod generator;
mod trace_id;
mod validator;
