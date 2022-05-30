use elfo_macros::message;

/// The command to reload configs and send changed ones.
/// By default, up-to-date configs aren't resent across the system.
/// Use `ReloadConfigs::with_force(true)` to change this behavior.
#[message(elfo = elfo_core)]
#[derive(Default)]
pub struct ReloadConfigs {
    pub(crate) force: bool,
}

impl ReloadConfigs {
    pub(crate) fn forcing() -> Self {
        Self { force: true }
    }

    /// If enabled, all configs will be updated, including up-to-date ones.
    pub fn with_force(self, force: bool) -> Self {
        ReloadConfigs { force }
    }
}

/// The request to reload configs and send changed ones.
/// If the validation stage is failed, `TryReloadConfigsRejected` is returned.
/// By default, up-to-date configs isn't resent across the system.
/// Use `TryReloadConfigs::with_force(true)` to change this behavior.
#[message(ret = Result<(), TryReloadConfigsRejected>, elfo = elfo_core)]
#[derive(Default)]
pub struct TryReloadConfigs {
    pub(crate) force: bool,
}

impl TryReloadConfigs {
    /// If enabled, all configs will be updated, including up-to-date ones.
    pub fn with_force(self, force: bool) -> Self {
        TryReloadConfigs { force }
    }
}

/// The response to `TryReloadConfigs`.
#[message(part, elfo = elfo_core)]
#[non_exhaustive]
pub struct TryReloadConfigsRejected {
    /// All reasons why configs cannot be updated.
    pub errors: Vec<ReloadConfigsError>,
}

/// Contains a reason why some actor rejects the config.
#[message(part, elfo = elfo_core)]
#[non_exhaustive]
pub struct ReloadConfigsError {
    // TODO: meta
    pub reason: String,
}
