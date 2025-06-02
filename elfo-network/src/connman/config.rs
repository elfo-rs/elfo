use std::time::Duration;

#[derive(Debug)]
pub(crate) struct Config {
    pub(crate) reconnect_interval: Duration,
}
