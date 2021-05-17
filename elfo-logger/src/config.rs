use std::path::PathBuf;

use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub(crate) struct Config {
    #[serde(default)]
    pub(crate) sink: Sink,
    pub(crate) path: Option<PathBuf>,
    // TODO: colors
}

#[derive(Debug, PartialEq, Deserialize)]
pub(crate) enum Sink {
    File,
    Stdout,
    // TODO: stdout + stderr
}

impl Default for Sink {
    fn default() -> Self {
        Sink::Stdout
    }
}
