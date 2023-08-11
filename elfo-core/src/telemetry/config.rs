use std::fmt;

use regex::Regex;
use serde::{
    de::{Deserializer, Error},
    Deserialize,
};

#[derive(Debug, Deserialize)]
#[serde(default)]
pub(crate) struct TelemetryConfig {
    pub(crate) per_actor_group: bool,
    pub(crate) per_actor_key: PerActorKey,
}

pub(crate) enum PerActorKey {
    Bool(bool),
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
