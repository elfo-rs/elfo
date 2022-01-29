pub(crate) use self::config::LoggingConfig;

mod config;
mod control;
mod filter;

// TODO: use `stability` instead.
#[doc(hidden)]
pub mod _priv {
    pub use super::control::{CheckResult, LoggingControl};
}
