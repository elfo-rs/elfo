#![allow(unreachable_pub)] // docsrs

use std::path::PathBuf;

use fxhash::FxHashMap;
use serde::{Deserialize, Deserializer};
use tracing::metadata::LevelFilter;

use bytesize::ByteSize;

#[derive(Debug, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub sink: Sink,
    pub path: Option<PathBuf>,
    #[serde(default)]
    pub format: Format,

    #[serde(default = "default_max_line_size")]
    pub max_line_size: ByteSize,

    #[serde(default)]
    pub targets: FxHashMap<String, LoggingTargetConfig>,
}

#[derive(Debug, Deserialize)]
pub struct LoggingTargetConfig {
    #[serde(deserialize_with = "deserialize_level_filter")]
    pub max_level: LevelFilter,
}

#[derive(Debug, Default, PartialEq, Deserialize)]
pub enum Sink {
    File,
    #[default]
    Stdout,
    // TODO: stdout + stderr
}

#[derive(Debug, Deserialize, Default)]
pub struct Format {
    #[serde(default)]
    pub with_location: bool,
    #[serde(default)]
    pub with_module: bool,
    // TODO: colors
}

fn default_max_line_size() -> ByteSize {
    ByteSize(u64::MAX)
}

// TODO: deduplicate with core
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
