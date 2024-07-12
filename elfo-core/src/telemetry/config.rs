//! [Config].
//!
//! [Config]: TelemetryConfig

use std::fmt;

use regex::Regex;
use serde::{
    de::{Deserializer, Error},
    Deserialize,
};

/// Telemetry configuration.
///
/// # Example
/// ```toml
/// [some_group]
/// system.telemetry.per_actor_group = false
/// system.teleemtry.per_actor_key = true
/// ```
#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct TelemetryConfig {
    /// Whether to enable per-actor-group telemetry.
    ///
    /// `true` by default.
    pub per_actor_group: bool,
    /// Whether to enable per-actor-key telemetry.
    ///
    /// Can be either `true` to produce metrics for all keys
    /// or regex pattern to combine keys into "groups".
    ///
    /// `false` by default.
    ///
    /// # Example
    /// ```toml
    /// per_actor_key = true # emit metrics keywise
    /// per_actor_key = [".*:(.*?)", "${1}"] # group keys
    /// ```
    pub per_actor_key: PerActorKey,
}

/// How to produce metrics for actor keys.
pub enum PerActorKey {
    /// Produce metrics for all keys.
    Bool(bool),
    /// Combine keys into "groups" by pattern.
    Replacement(Regex, String),
}

impl fmt::Debug for PerActorKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bool(flag) => write!(f, "{:?}", flag),
            Self::Replacement(pattern, template) => f
                .debug_tuple("")
                .field(&pattern.as_str())
                .field(&template)
                .finish(),
        }
    }
}

impl PerActorKey {
    pub(crate) fn is_enabled(&self) -> bool {
        match self {
            Self::Bool(flag) => *flag,
            Self::Replacement(_, _) => true,
        }
    }

    pub(crate) fn key(&self, actor_key: &str) -> Option<String> {
        match self {
            Self::Bool(_) => None,
            Self::Replacement(pattern, template) => {
                Some(pattern.replace(actor_key, template).into())
            }
        }
    }
}

impl<'de> Deserialize<'de> for PerActorKey {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum BoolOrPairOfStrings {
            Bool(bool),
            Pair(String, String),
        }

        Ok(match BoolOrPairOfStrings::deserialize(deserializer)? {
            BoolOrPairOfStrings::Bool(flag) => PerActorKey::Bool(flag),
            BoolOrPairOfStrings::Pair(pattern, template) => {
                let pattern = Regex::new(&pattern).map_err(D::Error::custom)?;
                PerActorKey::Replacement(pattern, template)
            }
        })
    }
}

impl Default for TelemetryConfig {
    fn default() -> Self {
        Self {
            per_actor_group: true,
            per_actor_key: PerActorKey::Bool(false),
        }
    }
}
