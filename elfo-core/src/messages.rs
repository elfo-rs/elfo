use std::fmt::Display;

use derive_more::Constructor;

use elfo_macros::message;

use crate::config::AnyConfig;

#[message(ret = (), elfo = crate)]
pub struct Ping;

#[message(ret = Result<(), ConfigRejected>, elfo = crate)]
#[derive(Constructor)]
pub struct ValidateConfig {
    pub config: AnyConfig,
}

#[message(ret = Result<(), ConfigRejected>, elfo = crate)]
#[derive(Constructor)]
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
pub struct ConfigUpdated {
    // TODO: add `old_config`.
}

#[message(elfo = crate)]
#[derive(Default)]
pub struct Terminate {
    pub(crate) closing: bool,
}

impl Terminate {
    pub(crate) fn closing() -> Self {
        Self { closing: true }
    }
}
