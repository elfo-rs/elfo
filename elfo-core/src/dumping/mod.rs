//! Includes structs and functions to work with dumping.
//! For more details see [The Actoromicon](https://actoromicon.rs/ch05-03-dumping.md.html).

// Stable.
pub use self::hider::hide;

// Unstable.
#[cfg(feature = "unstable")] // TODO: patch `stability`, again.
pub use self::{
    control::{CheckResult, DumpingControl},
    dump::{Direction, Dump, ErasedMessage, MessageKind, MessageName, Timestamp},
    dumper::Dumper,
    extract_name::{extract_name, extract_name_by_type},
    raw::Raw,
    recorder::{set_make_recorder, Recorder},
    sequence_no::SequenceNo,
};
#[cfg(not(feature = "unstable"))] // TODO: patch `stability`, again.
pub(crate) use self::{
    control::{CheckResult, DumpingControl},
    dump::{Direction, Dump, ErasedMessage, MessageKind, MessageName, Timestamp},
    dumper::Dumper,
    extract_name::{extract_name, extract_name_by_type},
    raw::Raw,
    recorder::{set_make_recorder, Recorder},
    sequence_no::SequenceNo,
};

pub(crate) use self::config::DumpingConfig;

#[cfg_attr(docsrs, doc(cfg(feature = "unstable")))]
#[stability::unstable]
pub const INTERNAL_CLASS: &str = "internal";

mod config;
mod control;
mod dump;
mod dumper;
mod extract_name;
mod hider;
mod raw;
mod recorder;
mod sequence_no;

// TODO: move to `scope::set_serde_env(SerdeMode::Dumping)`
//#[doc(hidden)]
#[stability::unstable]
pub fn set_in_dumping(flag: bool) {
    hider::set_in_dumping(flag);
}
