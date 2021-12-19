//! Includes `TraceId` and useful utilities around it.
//! For more details see [The Actoromicon](https://actoromicon.rs/ch05-04-tracing.html).

pub use self::trace_id::TraceId;

impl TraceId {
    /// Generates a new trace id according to [the schema](https://actoromicon.rs/ch05-04-tracing.html#traceid).
    pub fn generate() -> Self {
        self::generator::generate_trace_id()
    }
}

mod generator;
mod trace_id;
