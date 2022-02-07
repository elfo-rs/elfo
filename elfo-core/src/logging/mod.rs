pub(crate) use self::config::LoggingConfig;

mod config;
mod control;
mod filter;

// TODO: use `stability` instead.
#[doc(hidden)]
pub mod _priv {
    #[cfg(feature = "unstable")] // TODO: patch `stability`
    pub use super::control::{CheckResult, LoggingControl};
    #[cfg(not(feature = "unstable"))]
    pub(crate) use super::control::{CheckResult, LoggingControl};
}
