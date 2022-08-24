#![warn(rust_2018_idioms, unreachable_pub)]

use std::{
    future::Future,
    path::{Path, PathBuf},
    time::Duration,
};

use futures::future;
use fxhash::FxHashMap;
use serde::{de::Deserializer, Deserialize};
use serde_value::Value;
use tokio::{fs, select, time};
use tracing::{debug, error, info, warn};

use elfo_core as elfo;
use elfo_macros::msg_raw as msg;

use elfo::{
    config::AnyConfig,
    errors::RequestError,
    messages::{ConfigRejected, Ping, UpdateConfig, ValidateConfig},
    signal::{Signal, SignalKind},
    ActorGroup, ActorStatus, Addr, Context, Request, Schema, Topology,
};

pub use self::protocol::*;

mod helpers;
mod protocol;

// How often warn if a group is updating a config too long.
const WARN_INTERVAL: Duration = Duration::from_secs(5);

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

struct Configurer {
    ctx: Context,
    topology: Topology,
    source: ConfigSource,
    /// Stores hashes of configs per group.
    versions: FxHashMap<String, u64>,
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
        let can_start = self.load_and_update_configs(true).await.is_ok();

        if !can_start {
            panic!("configs are invalid at startup");
        }

        while let Some(envelope) = ctx.recv().await {
            msg!(match envelope {
                ReloadConfigs { force } => {
                    let _ = self.load_and_update_configs(force).await;
                }
                (TryReloadConfigs { force }, token) => {
                    let response = self
                        .load_and_update_configs(force)
                        .await
                        .map_err(|errors| TryReloadConfigsRejected { errors });

                    ctx.respond(token, response);
                }
            })
        }
    }

    async fn load_and_update_configs(
        &mut self,
        force: bool,
    ) -> Result<(), Vec<ReloadConfigsError>> {
        let config = match &self.source {
            ConfigSource::File(path) => {
                info!(message = "loading a config", path = %path.to_string_lossy());
                load_raw_config(path).await
            }
            ConfigSource::Fixture(value) => {
                info!("using a fixture");
                value.clone()
            }
        };

        let config = match config {
            Ok(config) => config,
            Err(error) => {
                error!(%error, "invalid config");
                return Err(vec![ReloadConfigsError { reason: error }]);
            }
        };

        let configs = match Deserialize::deserialize(config) {
            Ok(configs) => configs,
            Err(error) => {
                error!(%error, "invalid config");
                return Err(vec![ReloadConfigsError {
                    reason: error.to_string(),
                }]);
            }
        };

        let mut updated_groups = Vec::new();

        debug!("updating configs of system groups");
        updated_groups.extend(
            self.update_configs(&configs, TopologyFilter::System, force)
                .await?,
        );
        debug!("updating configs of user groups");
        updated_groups.extend(
            self.update_configs(&configs, TopologyFilter::User, force)
                .await?,
        );

        if updated_groups.is_empty() {
            info!("all groups' configs are up-to-date, nothing to update");
        } else {
            info!(
                message = "groups' configs are updated",
                groups = ?updated_groups,
            );
        }

        Ok(())
    }

    async fn update_configs(
        &mut self,
        configs: &FxHashMap<String, Value>,
        filter: TopologyFilter,
        force: bool,
    ) -> Result<Vec<String>, Vec<ReloadConfigsError>> {
        let mut config_list = match_configs(&self.topology, configs, filter);

        // Filter up-to-date configs if needed.
        if !force {
            config_list.retain(|c| {
                self.versions
                    .get(&c.group_name)
                    .map_or(true, |v| c.hash != *v)
            });
        }

        if config_list.is_empty() {
            return Ok(Vec::new());
        }

        // Validation.
        let status = ActorStatus::NORMAL.with_details("config validation");
        self.ctx.set_status(status);

        if let Err(errors) = self
            .request_all(&config_list, |config| ValidateConfig { config })
            .await
        {
            error!("config validation failed");
            self.ctx.set_status(ActorStatus::NORMAL);
            return Err(errors);
        }

        // Updating.
        let status = ActorStatus::NORMAL.with_details("config updating");
        self.ctx.set_status(status);

        if let Err(errors) = self
            .request_all(&config_list, |config| UpdateConfig { config })
            .await
        {
            error!("config updating failed");
            self.ctx
                .set_status(ActorStatus::ALARMING.with_details("possibly incosistent configs"));
            return Err(errors);
        }

        self.ctx.set_status(ActorStatus::NORMAL);

        // Update versions.
        self.versions
            .extend(config_list.iter().map(|c| (c.group_name.clone(), c.hash)));

        Ok(config_list.into_iter().map(|c| c.group_name).collect())
    }

    async fn request_all<R>(
        &self,
        config_list: &[ConfigWithMeta],
        make_msg: impl Fn(AnyConfig) -> R,
    ) -> Result<(), Vec<ReloadConfigsError>>
    where
        R: Request<Response = Result<(), ConfigRejected>>,
    {
        let futures = config_list
            .iter()
            .cloned()
            .map(|item| {
                let group = item.group_name;
                let fut = self
                    .ctx
                    .request_to(item.addr, make_msg(item.config))
                    .all()
                    .resolve();

                wrap_request_future(fut, group)
            })
            .collect::<Vec<_>>();

        // TODO: use `try_join_all`.
        let errors = future::join_all(futures)
            .await
            .into_iter()
            .flat_map(|(group, results)| results.into_iter().map(move |res| (group.clone(), res)))
            .filter_map(|(group, result)| match result {
                Ok(Ok(_)) | Err(_) => None,
                Ok(Err(reject)) => Some((group, reject.reason)),
                // TODO: it's meaningful, but doesn't work well with empty groups.
                // Err(RequestError::Closed(_)) => Some((group, "some group is closed".into())),
            })
            // TODO: provide more info.
            .inspect(|(group, reason)| error!(%group, %reason, "invalid config"))
            .map(|(_, reason)| ReloadConfigsError { reason })
            .collect::<Vec<_>>();

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

async fn wrap_request_future<F: Future>(f: F, group_name: String) -> (String, F::Output) {
    tokio::pin!(f);
    loop {
        select! {
            output = &mut f => return (group_name, output),
            _ = time::sleep(WARN_INTERVAL) => {
                warn!(group = %group_name, "group is updating the config suspiciously long");
            }
        }
    }
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

enum TopologyFilter {
    System,
    User,
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
