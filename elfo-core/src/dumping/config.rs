//! [Config].
//!
//! [Config]: DumpingConfig

use serde::Deserialize;

/// Dumping configuration.
///
/// # Example
/// ```toml
/// [some_group]
/// system.dumping.disabled = false
/// system.dumping.max_rate = 1_000
/// ```
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct DumpingConfig {
    /// Whether dumping is disabled.
    ///
    /// `false` by default.
    pub disabled: bool,
    /// Maximum rate of dumping.
    /// Exceeding this rate will cause messages to not be dumped.
    ///
    /// `100_000` by default.
    pub max_rate: u64,
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
