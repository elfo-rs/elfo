// REVIEW: remove before merge
#![allow(dead_code)]

use std::path::PathBuf;

use fxhash::FxHashMap;
use serde::{Deserialize, Deserializer};
use tracing::metadata::LevelFilter;

#[derive(Debug, Deserialize)]
pub(crate) struct Config {
    #[serde(default)]
    pub(crate) sink: Sink,
    pub(crate) path: Option<PathBuf>,
    #[serde(default)]
    pub(crate) format: Format,

    #[serde(deserialize_with = "deserialize_level_filter", default = "default_max_level")]
    pub(crate) max_level: LevelFilter,
    #[serde(default)]
    pub(crate) targets: FxHashMap<String, LoggingTargetConfig>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct LoggingTargetConfig {
    #[serde(deserialize_with = "deserialize_level_filter")]
    pub(crate) max_level: LevelFilter,
}

#[derive(Debug, Default, PartialEq, Deserialize)]
pub(crate) enum Sink {
    File,
    #[default]
    Stdout,
    // TODO: stdout + stderr
}

#[derive(Debug, Deserialize, Default)]
pub(crate) struct Format {
    #[serde(default)]
    pub(crate) with_location: bool,
    #[serde(default)]
    pub(crate) with_module: bool,
    // TODO: colors
}

fn default_max_level() -> LevelFilter {
    LevelFilter::INFO
}

// REVIEW: deduplicate with core
fn deserialize_level_filter<'de, D>(deserializer: D) -> Result<LevelFilter, D::Error>
where
    D: Deserializer<'de>,
{
    use PrettyLevelFilter::*;

    #[derive(Deserialize)]
    pub(crate) enum PrettyLevelFilter {
        Trace,
        Debug,
        Info,
        Warn,
        Error,
        Off,
    }

    let pretty = PrettyLevelFilter::deserialize(deserializer)?;

    Ok(match pretty {
        Trace => LevelFilter::TRACE,
        Debug => LevelFilter::DEBUG,
        Info => LevelFilter::INFO,
        Warn => LevelFilter::WARN,
        Error => LevelFilter::ERROR,
        Off => LevelFilter::OFF,
    })
}
