//! Configuration for the logger.
//!
//! Note: all types here are exported only for documentation purposes
//! and are not subject to stable guarantees. However, the config
//! structure (usually encoded in TOML) follows stable guarantees.

use std::path::PathBuf;

use fxhash::FxHashMap;
use serde::{Deserialize, Deserializer};
use tracing::metadata::LevelFilter;

use bytesize::ByteSize;

/// Logger configuration.
///
/// It's exported only for documentation purposes and cannot be created or
/// received outside the dumper.
#[derive(Debug, Deserialize)]
pub struct Config {
    /// Sink for the log output.
    /// By default logs are written to stdout.
    #[serde(default)]
    pub sink: Sink,
    /// Path to the log file, applicable only for `Sink::File`.
    pub path: Option<PathBuf>,
    /// Log format.
    #[serde(default)]
    pub format: Format,

    /// Size limit for each written log-line, in bytes.
    /// If size exceeds the limit, it will be truncated in the following order:
    ///
    /// 1. Message with custom fields
    /// 2. Meta-info (level, timestamp, ...)
    /// 3. Meta-fields (location, module)
    #[serde(default = "default_max_line_size")]
    pub max_line_size: ByteSize,

    /// Override log levels for specific targets.
    /// Useful to suppress noisy logs from dependencies.
    #[serde(default)]
    pub targets: FxHashMap<String, LoggingTargetConfig>,
}

/// Configuration for a specific logging target.
#[derive(Debug, Deserialize)]
pub struct LoggingTargetConfig {
    /// Maximum log level for the target.
    #[serde(deserialize_with = "deserialize_level_filter")]
    pub max_level: LevelFilter,
}

/// Sink for the log output.
/// By default logs are written to stdout.
#[derive(Debug, Default, PartialEq, Deserialize)]
pub enum Sink {
    /// Write logs to a file, specified by `path`.
    File,
    /// Write logs to stdout.
    #[default]
    Stdout,
    // TODO: stdout + stderr
}

/// Log format.
#[derive(Debug, Deserialize, Default)]
pub struct Format {
    /// Include location info in the log output.
    #[serde(default)]
    pub with_location: bool,
    /// Include module info in the log output.
    #[serde(default)]
    pub with_module: bool,
    /// Specify when to use colored output.
    /// By default, colors are enabled only for interactive terminals.
    #[serde(default)]
    pub colorization: Colorization,
}

/// When to use colored output.
#[derive(Debug, Deserialize, Default)]
pub enum Colorization {
    /// Use colors only if the output is a terminal.
    #[default]
    Auto,
    /// Never use colors.
    Never,
    /// Use colors unconditionally.
    Always,
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
