//! [Config].
//!
//! [Config]: DumpingConfig

use fxhash::FxHashMap;
use serde::Deserialize;

use crate::dumping::Level;

/// Dumping configuration.
///
/// # Example
/// ```toml
/// [some_group]
/// system.dumping.max_level = "Normal"
/// system.dumping.max_rate_per_class = 10000
///
/// system.dumping.classes.http = { max_level = "Verbose", max_rate = 1000 }
/// ```
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub(crate) struct DumpingConfig {
    /// TODO
    pub(crate) max_level: LevelFilter,
    /// TODO
    #[serde(alias = "max_rate")]
    pub(crate) max_rate_per_class: u64,
    /// TODO
    pub(crate) classes: FxHashMap<String, DumpingClassConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct DumpingClassConfig {
    pub(crate) max_level: LevelFilter,
    pub(crate) max_rate: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) enum LevelFilter {
    Off,
    Normal,
    Verbose,
    Total,
}

impl LevelFilter {
    pub(crate) fn into_level(&self) -> Option<Level> {
        match self {
            Self::Off => None,
            Self::Normal => Some(Level::Normal),
            Self::Verbose => Some(Level::Verbose),
            Self::Total => Some(Level::Total),
        }
    }
}

impl Default for DumpingConfig {
    fn default() -> Self {
        Self {
            max_level: LevelFilter::Off,
            max_rate_per_class: 10_000,
            classes: FxHashMap::default(),
        }
    }
}
