#[cfg(unix)]
use std::path::PathBuf;
use std::{str::FromStr, time::Duration};

use derive_more::Display;
use eyre::{bail, Result};
use serde::{
    de::{self, Deserializer},
    Deserialize, Serialize,
};

#[derive(Debug, Deserialize)]
pub(crate) struct Config {
    pub(crate) listen: Vec<Transport>,
    #[serde(with = "humantime_serde", default = "default_ping_interval")]
    pub(crate) ping_interval: Duration,
    #[serde(default)]
    pub(crate) discovery: DiscoveryConfig, // TODO: optional?
    #[serde(default)]
    pub(crate) compression: CompressionConfig,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct CompressionConfig {
    #[serde(default)]
    pub(crate) algorithm: CompressionAlgorithm,
}

#[derive(Debug, Default, PartialEq, Eq, Deserialize)]
pub(crate) enum CompressionAlgorithm {
    Lz4,
    #[default]
    None,
}

fn default_ping_interval() -> Duration {
    Duration::from_secs(5)
}

#[derive(Debug, Deserialize, Default)]
pub(crate) struct DiscoveryConfig {
    pub(crate) predefined: Vec<Transport>,
    #[serde(with = "humantime_serde", default = "default_attempt_interval")]
    pub(crate) attempt_interval: Duration,
}

fn default_attempt_interval() -> Duration {
    Duration::from_secs(10)
}

#[derive(Debug, Clone, Hash, PartialEq, Eq, Display, Serialize)]
pub(crate) enum Transport {
    #[display(fmt = "tcp://{}", _0)]
    Tcp(String),
    #[cfg(unix)]
    #[display(fmt = "uds://{}", "_0.display()")]
    Uds(PathBuf),
}

impl FromStr for Transport {
    type Err = eyre::Error;

    fn from_str(s: &str) -> Result<Self> {
        #[cfg(unix)]
        const PROTOCOLS: &str = "tcp or uds";
        #[cfg(not(unix))]
        const PROTOCOLS: &str = "tcp";

        let (protocol, addr) = s.split_once("://").unwrap_or_default();

        match protocol {
            "" => bail!("protocol must be specified ({PROTOCOLS})"),
            "tcp" => Ok(Transport::Tcp(addr.into())),
            #[cfg(unix)]
            "uds" => {
                eyre::ensure!(
                    !addr.ends_with('/'),
                    "path to UDS socket cannot be directory"
                );
                Ok(Transport::Uds(PathBuf::from(addr)))
            }
            proto => bail!("unknown protocol: {proto}"),
        }
    }
}

impl<'de> Deserialize<'de> for Transport {
    fn deserialize<D>(deserializer: D) -> Result<Transport, D::Error>
    where
        D: Deserializer<'de>,
    {
        // FIXME: cannot use `&str` here: `expected borrowed string`.
        let s: String = Deserialize::deserialize(deserializer)?;

        s.parse::<Transport>()
            .map_err(|err| de::Error::custom(format!(r#"unsupported transport: "{}", {}"#, s, err)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transport_parsing() {
        // Missing protocol
        assert!(Transport::from_str("")
            .unwrap_err()
            .to_string()
            .starts_with("protocol must be specified"));
        assert!(Transport::from_str("://a/b")
            .unwrap_err()
            .to_string()
            .starts_with("protocol must be specified"));

        // Unknown protocol
        assert!(Transport::from_str("foo://a")
            .unwrap_err()
            .to_string()
            .starts_with("unknown protocol"));
        #[cfg(not(unix))]
        assert!(Transport::from_str("uds://a")
            .unwrap_err()
            .to_string()
            .starts_with("unknown protocol"));

        // TCP
        assert_eq!(
            Transport::from_str("tcp://127.0.0.1:4242").unwrap(),
            Transport::Tcp("127.0.0.1:4242".into())
        );

        // UDS
        #[cfg(unix)]
        {
            assert_eq!(
                Transport::from_str("uds:///a/b").unwrap(),
                Transport::Uds("/a/b".into())
            );
            assert_eq!(
                Transport::from_str("uds://rel/a/b").unwrap(),
                Transport::Uds("rel/a/b".into())
            );
            assert_eq!(
                Transport::from_str("uds:///a/").unwrap_err().to_string(),
                "path to UDS socket cannot be directory"
            );
        }
    }
}
