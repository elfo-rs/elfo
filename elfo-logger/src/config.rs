use std::path::PathBuf;

use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub(crate) struct Config {
    #[serde(default)]
    pub(crate) sink: Sink,
    pub(crate) path: Option<PathBuf>,
    #[serde(default)]
    pub(crate) format: Format,
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
