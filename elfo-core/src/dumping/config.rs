use serde::Deserialize;

#[derive(Clone, Deserialize)]
#[serde(default)]
pub(crate) struct DumpingConfig {
    pub(crate) disabled: bool,
    pub(crate) max_rate: u64,
    // TODO: per class overrides.
}

impl Default for DumpingConfig {
    fn default() -> Self {
        Self {
            disabled: false,
            max_rate: 100_000,
        }
    }
}
