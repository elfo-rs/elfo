use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub(crate) struct Config {
    pub(crate) listener: ListenerConfig,
    #[serde(default)]
    pub(crate) discovery: DiscoveryConfig, // TODO: optional?
}

#[derive(Debug, Deserialize)]
pub(crate) struct ListenerConfig {
    pub(crate) tcp: String,
}

#[derive(Debug, Deserialize, Default)]
pub(crate) struct DiscoveryConfig {
    pub(crate) predefined: Vec<String>,
}
