use elfo_core::message;

/// The request to reload configs and send changed ones.
/// If the validation stage is failed, `ReloadConfigsRejected` is returned.
/// By default, up-to-date configs isn't resent across the system.
/// Use `ReloadConfigs::forcing()` to change this behavior.
#[message(ret = Result<(), ReloadConfigsRejected>)]
#[derive(Default)]
pub struct ReloadConfigs {
    pub(crate) force: bool,
}

impl ReloadConfigs {
    /// All configs will be updated, including up-to-date ones.
    pub fn forcing() -> Self {
        Self { force: true }
    }
}

/// The response to `ReloadConfigs`.
#[message(part)]
#[non_exhaustive]
pub struct ReloadConfigsRejected {
    /// All reasons why configs cannot be updated.
    pub errors: Vec<ReloadConfigsError>,
}

/// Contains a reason why some actor rejects the config.
#[message(part)]
#[non_exhaustive]
pub struct ReloadConfigsError {
    pub group: String,
    pub reason: String,
}
