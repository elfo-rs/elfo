use std::fmt::Display;

use elfo_macros::message;

use crate::config::AnyConfig;

#[message(ret = Result<(), ConfigRejected>, elfo = crate)]
#[non_exhaustive]
pub struct ValidateConfig {
    pub config: AnyConfig,
}

#[message(ret = Result<ConfigUpdated, ConfigRejected>, elfo = crate)]
pub struct UpdateConfig {
    pub config: AnyConfig,
}

#[message(elfo = crate)]
pub struct ConfigRejected {
    pub reason: String,
}

impl<R: Display> From<R> for ConfigRejected {
    fn from(reason: R) -> Self {
        Self {
            reason: reason.to_string(),
        }
    }
}

#[message(elfo = crate)]
#[non_exhaustive]
pub struct ConfigUpdated {
    // TODO: add `old_config`.
}
