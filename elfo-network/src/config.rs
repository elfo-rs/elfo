use std::{net::SocketAddr, time::Duration};

use derive_more::Display;
use serde::{
    de::{self, Deserializer},
    Deserialize, Serialize,
};

#[derive(Debug, Deserialize)]
pub(crate) struct Config {
    pub(crate) listen: Vec<Transport>,
    #[serde(default)]
    pub(crate) discovery: DiscoveryConfig, // TODO: optional?
}

#[derive(Debug, Deserialize, Default)]
pub(crate) struct DiscoveryConfig {
    pub(crate) predefined: Vec<Transport>,
    #[serde(with = "humantime_serde", default = "default_attempt_interval")]
    pub(crate) attempt_interval: Duration,
}

fn default_attempt_interval() -> Duration {
    Duration::from_secs(60)
}

#[derive(Debug, Clone, Hash, PartialEq, Eq, Display, Serialize)]
pub(crate) enum Transport {
    #[display(fmt = "tcp://{}", _0)]
    Tcp(SocketAddr),
}

impl<'de> Deserialize<'de> for Transport {
    fn deserialize<D>(deserializer: D) -> Result<Transport, D::Error>
    where
        D: Deserializer<'de>,
    {
        // FIXME: cannot use `&str` here: `expected borrowed string`.
        let s: String = Deserialize::deserialize(deserializer)?;

        parse_transport(&s)
            .map_err(|err| de::Error::custom(format!(r#"unsupported transport: "{}", {}"#, s, err)))
    }
}

fn parse_transport(s: &str) -> Result<Transport, &'static str> {
    if !s.contains("://") {
        return Err(r#"protocol must be specified (e.g. "tcp://")"#);
    }

    if let Some(addr) = s.strip_prefix("tcp://") {
        addr.parse()
            .map(Transport::Tcp)
            .map_err(|_| "invalid TCP address")
    } else {
        Err("unknown protocol")
    }
}
