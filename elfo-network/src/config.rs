//! Configuration for the network actors.
//!
//! Note: all types here are exported only for documentation purposes
//! and are not subject to stable guarantees. However, the config
//! structure (usually encoded in TOML) follows stable guarantees.

#[cfg(unix)]
use std::path::PathBuf;
use std::{str::FromStr, time::Duration};

use derive_more::Display;
use eyre::{bail, Result};
use serde::{
    de::{self, Deserializer},
    Deserialize, Serialize,
};

/// The network actors' config.
///
/// # Examples
/// ```toml
/// [system.network]
/// listen = ["tcp://0.0.0.1:8150"]
/// discovery.predefined = [
///     "tcp://localhost:4242",
///     "uds:///tmp/sock"
/// ]
/// ```
#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    /// A list of addresses to listen on.
    #[serde(default)]
    pub listen: Vec<Transport>,
    /// How to discover other nodes.
    #[serde(default)]
    pub discovery: DiscoveryConfig, // TODO: optional?
    /// Compression settings.
    #[serde(default)]
    pub compression: CompressionConfig,
    /// How often nodes should ping each other.
    ///
    /// Pings are used to measure RTT and detect dead connections.
    /// For the latest purpose, see `idle_timeout`.
    ///
    /// `5s` by default.
    #[serde(with = "humantime_serde", default = "default_ping_interval")]
    pub ping_interval: Duration,
    /// The maximum inactivity time of every connection.
    ///
    /// If no data is received on a connection for over `idle_timeout` time,
    /// the connection is considered dead and will be automatically closed.
    ///
    /// This timeout is checked every `ping_interval` time, so the actual time
    /// lies in the range of `idle_timeout` to `idle_timeout + ping_interval`.
    ///
    /// `30s` by default.
    #[serde(with = "humantime_serde", default = "default_idle_timeout")]
    pub idle_timeout: Duration,
}

/// Preference.
#[derive(Debug, Clone, Copy, Default, Deserialize)]
pub enum Preference {
    /// This is preferred, implies [`Preference::Supported`].
    Preferred,

    /// This is just supported.
    #[default]
    Supported,

    /// Must not be used.
    Disabled,
}

/// Options for the specific algorithm.
#[derive(Debug, Deserialize, Clone)]
pub struct Algorithm {
    /// Preference when deciding which algorithm to use in
    /// communication between nodes.
    pub preference: Preference,
}

/// Options for the compression algorithms.
#[derive(Debug, Deserialize, Clone)]
pub struct CompressionAlgorithms {
    /// LZ4 compression algorithm.
    #[serde(default = "CompressionAlgorithms::lz4_default")]
    pub lz4: Algorithm,
}

impl CompressionAlgorithms {
    const fn lz4_default() -> Algorithm {
        Algorithm {
            preference: Preference::Supported,
        }
    }
}

impl Default for CompressionAlgorithms {
    fn default() -> Self {
        Self {
            lz4: Self::lz4_default(),
        }
    }
}

/// Compression settings.
#[derive(Debug, Default, Deserialize, Clone)]
pub struct CompressionConfig {
    /// Compression algorithms.
    ///
    /// Example:
    /// ```toml
    /// algorithms.lz4 = { preference = "Preferred" }
    /// ```
    ///
    /// Preferred implies supported.
    #[serde(default)]
    pub algorithms: CompressionAlgorithms,
}

fn default_ping_interval() -> Duration {
    Duration::from_secs(5)
}

fn default_idle_timeout() -> Duration {
    Duration::from_secs(30)
}

/// How to discover other nodes.
#[derive(Debug, Deserialize, Clone, Default)]
pub struct DiscoveryConfig {
    /// Predefined list of transports to connect to.
    pub predefined: Vec<Transport>,
    /// How often to attempt to connect to other nodes.
    #[serde(with = "humantime_serde", default = "default_attempt_interval")]
    pub attempt_interval: Duration,
}

fn default_attempt_interval() -> Duration {
    Duration::from_secs(10)
}

/// Transport used for communication between nodes.
#[derive(Debug, Clone, Hash, PartialEq, Eq, Display, Serialize)]
pub enum Transport {
    /// TCP transport ("tcp://host:port").
    #[display("tcp://{_0}")]
    Tcp(String),
    /// Unix domain socket transport ("uds://path/to/socket").
    ///
    /// Used only on UNIX systems, ignored on other platforms.
    #[cfg(unix)]
    #[display("uds://{}", "_0.display()")]
    Uds(PathBuf),
    /// Turmoil v0.6 transport ("turmoil06://host").
    ///
    /// Useful for testing purposes only.
    #[cfg(feature = "turmoil06")]
    #[display("turmoil06://{_0}")]
    Turmoil06(String),
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
            #[cfg(feature = "turmoil06")]
            "turmoil06" => Ok(Transport::Turmoil06(addr.into())),
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
        assert_eq!(
            Transport::from_str("tcp://alice:4242").unwrap(),
            Transport::Tcp("alice:4242".into())
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

        // Turmoil06
        #[cfg(feature = "turmoil06")]
        assert_eq!(
            Transport::from_str("turmoil06://alice").unwrap(),
            Transport::Turmoil06("alice".into())
        );
    }
}
