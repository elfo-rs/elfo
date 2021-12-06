#![warn(rust_2018_idioms, unreachable_pub)]

use std::path::{Path, PathBuf};

use futures::{future, FutureExt};
use fxhash::FxHashMap;
use serde::{de::Deserializer, Deserialize};
use serde_value::Value;
use tokio::fs;
use tracing::error;

use elfo_core as elfo;
use elfo_macros::{message, msg_raw as msg};

use elfo::{
    config::AnyConfig,
    errors::RequestError,
    messages::{ConfigRejected, Ping, UpdateConfig, ValidateConfig},
    signal::{Signal, SignalKind},
    ActorGroup, ActorStatus, Addr, Context, Request, Schema, Topology,
};

mod helpers;

pub fn fixture(topology: &Topology, config: impl for<'de> Deserializer<'de>) -> Schema {
    let config = Value::deserialize(config).map_err(|err| err.to_string());
    let source = ConfigSource::Fixture(config);
    let topology = topology.clone();
    ActorGroup::new().exec(move |ctx| Configurer::new(ctx, topology.clone(), source.clone()).main())
}

pub fn from_path(topology: &Topology, path_to_config: impl AsRef<Path>) -> Schema {
    let source = ConfigSource::File(path_to_config.as_ref().to_path_buf());
    let topology = topology.clone();
    ActorGroup::new().exec(move |ctx| Configurer::new(ctx, topology.clone(), source.clone()).main())
}

#[derive(Clone)]
enum ConfigSource {
    File(PathBuf),
    Fixture(Result<Value, String>),
}

#[derive(Clone)]
struct ConfigWithMeta {
    group_name: String,
    addr: Addr,
    config: AnyConfig,
    hash: u64,
}

/// The command to reload configs and send changed ones.
/// By default, up-to-date configs isn't resent across the system.
/// Use `ReloadConfigs::with_force(true)` to change this behavior.
#[non_exhaustive]
#[message(elfo = elfo_core)]
#[derive(Default)]
pub struct ReloadConfigs {
    force: bool,
}

impl ReloadConfigs {
    fn forcing() -> Self {
        Self { force: true }
    }

    /// If enabled, all configs will be updated, including up-to-date ones.
    pub fn with_force(self, force: bool) -> Self {
        ReloadConfigs { force }
    }
}

struct Configurer {
    ctx: Context,
    topology: Topology,
    source: ConfigSource,
    /// Stores hashes of configs per group.
    versions: FxHashMap<String, u64>,
}

impl Configurer {
    fn new(ctx: Context, topology: Topology, source: ConfigSource) -> Self {
        Self {
            ctx,
            topology,
            source,
            versions: FxHashMap::default(),
        }
    }

    async fn main(mut self) {
        let hangup = Signal::new(SignalKind::Hangup, ReloadConfigs::default);
        let user2 = Signal::new(SignalKind::User2, ReloadConfigs::forcing);

        let mut ctx = self.ctx.clone().with(&hangup).with(user2);
        let can_start = self.load_and_update_configs(true).await;

        while let Some(envelope) = ctx.recv().await {
            msg!(match envelope {
                (Ping, token) if can_start => ctx.respond(token, ()),
                (Ping, token) => drop(token),
                ReloadConfigs { force } => {
                    self.load_and_update_configs(force).await;
                }
            })
        }
    }

    async fn load_and_update_configs(&mut self, force: bool) -> bool {
        let config = match &self.source {
            ConfigSource::File(path) => load_raw_config(path).await,
            ConfigSource::Fixture(value) => value.clone(),
        };

        let config = match config {
            Ok(config) => config,
            Err(error) => {
                error!(%error, "invalid config");
                return false;
            }
        };

        let configs = match Deserialize::deserialize(config) {
            Ok(configs) => configs,
            Err(error) => {
                error!(%error, "invalid config");
                return false;
            }
        };

        let system_updated = self
            .update_configs(&configs, TopologyFilter::System, force)
            .await;
        let user_udpated = self
            .update_configs(&configs, TopologyFilter::User, force)
            .await;

        system_updated && user_udpated
    }

    async fn update_configs(
        &mut self,
        configs: &FxHashMap<String, Value>,
        filter: TopologyFilter,
        force: bool,
    ) -> bool {
        let mut config_list = match_configs(&self.topology, configs, filter);

        // Filter up-to-date configs if needed.
        if !force {
            config_list.retain(|c| {
                self.versions
                    .get(&c.group_name)
                    .map_or(true, |v| c.hash != *v)
            });
        }

        // Validation.
        let status = ActorStatus::NORMAL.with_details("config validation");
        self.ctx.set_status(status);

        if !self
            .request_all(&config_list, |config| ValidateConfig { config })
            .await
        {
            error!("config validation failed");
            self.ctx.set_status(ActorStatus::NORMAL);
            return false;
        }

        // Updating.
        let status = ActorStatus::NORMAL.with_details("config updating");
        self.ctx.set_status(status);

        if !self
            .request_all(&config_list, |config| UpdateConfig { config })
            .await
        {
            error!("config updating failed");
            self.ctx
                .set_status(ActorStatus::ALARMING.with_details("possibly incosistent configs"));
            return false;
        }

        // TODO: make `Ping` automatically handled.
        // if !ping(ctx, &config_list).await {
        // error!("ping failed");
        // ctx.set_status(ActorStatus::ALARMING.with_details("possibly incosistent
        // configs")); return false;
        // }

        self.ctx.set_status(ActorStatus::NORMAL);

        // Update versions.
        self.versions
            .extend(config_list.into_iter().map(|c| (c.group_name, c.hash)));

        true
    }

    async fn request_all<R>(
        &self,
        config_list: &[ConfigWithMeta],
        make_msg: impl Fn(AnyConfig) -> R,
    ) -> bool
    where
        R: Request<Response = Result<(), ConfigRejected>>,
    {
        let futures = config_list
            .iter()
            .cloned()
            .map(|item| {
                let group = item.group_name;
                self.ctx
                    .request_to(item.addr, make_msg(item.config))
                    .all()
                    .resolve()
                    .map(|res| (group, res))
            })
            .collect::<Vec<_>>();

        // TODO: use `try_join_all`.
        let errors = future::join_all(futures)
            .await
            .into_iter()
            .map(|(group, results)| results.into_iter().map(move |res| (group.clone(), res)))
            .flatten()
            .filter_map(|(group, result)| match result {
                Ok(Ok(_)) | Err(_) => None,
                Ok(Err(reject)) => Some((group, reject.reason)),
                /* TODO: it's meaningful, but doesn't work well with empty groups.
                 * Err(RequestError::Closed(_)) => Some((group, "some group is closed".into())), */
            })
            // TODO: provide more info.
            .inspect(|(group, reason)| error!(%group, %reason, "invalid config"));

        errors.count() == 0
    }
}

enum TopologyFilter {
    System,
    User,
}

#[allow(dead_code)]
async fn ping(ctx: &Context, config_list: &[ConfigWithMeta]) -> bool {
    let futures = config_list
        .iter()
        .cloned()
        .map(|item| ctx.request_to(item.addr, Ping).all().resolve())
        .collect::<Vec<_>>();

    // TODO: use `try_join_all`.
    let errors = future::join_all(futures)
        .await
        .into_iter()
        .flatten()
        .filter_map(|result| match result {
            Ok(()) | Err(RequestError::Ignored) => None,
            Err(RequestError::Closed(_)) => Some(String::from("some group is closed")),
        })
        // TODO: provide more info.
        .inspect(|reason| error!(%reason, "ping failed"));

    errors.count() == 0
}

async fn load_raw_config(path: impl AsRef<Path>) -> Result<Value, String> {
    let content = fs::read_to_string(path)
        .await
        .map_err(|err| err.to_string())?;
    toml::from_str(&content).map_err(|err| err.to_string())
}

fn match_configs(
    topology: &Topology,
    configs: &FxHashMap<String, Value>,
    filter: TopologyFilter,
) -> Vec<ConfigWithMeta> {
    topology
        .actor_groups()
        // Entrypoints' configs are updated only at startup.
        .filter(|group| !group.is_entrypoint)
        .filter(|group| match filter {
            TopologyFilter::System => group.name.starts_with("system."),
            TopologyFilter::User => !group.name.starts_with("system."),
        })
        .map(|group| {
            let empty = Value::Map(Default::default());
            let common = helpers::get_config(configs, "common").unwrap_or(&empty);
            let config = helpers::get_config(configs, &group.name).cloned();
            let config = helpers::add_defaults(config, common);

            ConfigWithMeta {
                group_name: group.name.clone(),
                addr: group.addr,
                hash: fxhash::hash64(&config),
                config: AnyConfig::new(config),
            }
        })
        .collect()
}
