//! Configuration for the dumper.
//!
//! All types are exported only for documentation purposes, they cannot be
//! created or received outside the dumper.
//!
//! The main structure here is [`Config`].
#![allow(unreachable_pub)] // docsrs
use std::time::Duration;

use bytesize::ByteSize;
use serde::Deserialize;

/// The dumper's config.
///
/// It's exported only for documentation purposes and cannot be created or
/// received outside the dumper.
///
/// # Examples
/// Only `path` is required. Thus, a minimal config looks like
/// ```toml
/// [system.dumpers]
/// path = "/path/all.dump"
/// ```
/// It writes all dumps into the same file.
///
/// However, it's recommended to use file-per-class and enable per actor
/// telemetry.
/// ```toml
/// [system.dumpers]
/// system.telemetry.per_actor_group = false
/// system.telemetry.per_actor_key = true
///
/// path = "/path/{class}.dump"
/// rules = [
///     { max_size = "128KiB", on_overflow = "Truncate" },
///     { class = "ws_api", log_on_overflow = "Info" },
///     { class = "external", max_size = "1MiB" },
/// ]
/// ```
#[derive(Debug, Deserialize)]
pub struct Config {
    /// A path to a dump file or template:
    /// * `path/all.dump` - one file.
    /// * `path/{class}.dump` - file per class.
    pub path: String,
    /// How often dumpers should write dumps to files.
    /// `500ms` by default.
    #[serde(with = "humantime_serde", default = "default_write_interval")]
    pub write_interval: Duration,
    /// In order to avoid noisy logs about skipped, failed and truncated dumps,
    /// they are logged with this specified cooldown.
    /// `1m` by default.
    #[serde(with = "humantime_serde", default = "default_log_cooldown")]
    pub log_cooldown: Duration,
    /// The maximum number of dumps in memory per class. If exceeded, old
    /// dumps are dropped.
    /// `3_000_000` by default.
    #[serde(default = "default_registry_capacity")]
    pub registry_capacity: usize,
    /// Rule set to override properties.
    /// All rules that match a message are merged. If several relevant rules
    /// define same property, the last one is applied.
    ///
    /// There is the prepended implicit rule that defines default properties:
    /// ```toml
    /// { max_size = "64KiB", on_overflow = "Skip", log_on_overflow = "Warn", log_on_failure = "Warn" }
    /// ```
    #[serde(default)]
    pub rules: Vec<Rule>,
}

/// Defines a rule to override some properties.
///
/// It's exported only for documentation purposes and cannot be created or
/// received outside the dumper.
#[derive(Debug, Default, Clone, Deserialize, PartialEq, Eq)]
pub struct Rule {
    // Matchers.
    /// Applies only for the specified class.
    pub class: Option<String>,
    /// Applies only for the specified protocol.
    pub protocol: Option<String>,
    /// Applies only for the specified message.
    pub message: Option<String>,

    // Params.
    /// Specified the maximum size of a dump.
    pub max_size: Option<ByteSize>,
    /// Specified what to do if `max_size` is reached.
    pub on_overflow: Option<OnOverflow>,
    /// Specified the logging level if `max_size` is reached.
    pub log_on_overflow: Option<LogLevel>,
    /// Specified the logging level if a message cannot be serialized.
    pub log_on_failure: Option<LogLevel>,
}

/// What to do if a dump is too big.
///
/// It's exported only for documentation purposes and cannot be created or
/// received outside the dumper.
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
pub enum OnOverflow {
    /// Skip a dump, don't write to a file.
    Skip,
    /// Truncate a dump, serialize as an incomplete JSON with appended
    /// `TRUNCATED`.
    Truncate,
}

impl Config {
    pub(crate) fn path(&self, class: &str) -> String {
        self.path.replace("{class}", class)
    }
}

fn default_write_interval() -> Duration {
    Duration::from_millis(500)
}

fn default_log_cooldown() -> Duration {
    Duration::from_secs(60)
}

fn default_registry_capacity() -> usize {
    3_000_000
}

/// A logging level.
///
/// It's exported only for documentation purposes and cannot be created or
/// received outside the dumper.
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
    Off,
}
