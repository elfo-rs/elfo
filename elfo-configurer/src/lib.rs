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
use tracing::{error, info, warn};

use elfo_core::{
    config::AnyConfig,
    errors::RequestError,
    messages::{
        EntrypointError, Ping, StartEntrypoint, StartEntrypointRejected, UpdateConfig,
        ValidateConfig,
    },
    msg, scope,
    signal::{Signal, SignalKind},
    ActorGroup, ActorStatus, Addr, Blueprint, Context, Topology,
};

pub use self::protocol::*;

mod helpers;
mod protocol;

// How often warn if a group is updating a config too long.
const WARN_INTERVAL: Duration = Duration::from_secs(5);

pub fn fixture(topology: &Topology, config: impl for<'de> Deserializer<'de>) -> Blueprint {
    let config = Value::deserialize(config).map_err(|err| err.to_string());
    let source = ConfigSource::Fixture(config);
    let topology = topology.clone();
    ActorGroup::new().exec(move |ctx| Configurer::new(ctx, topology.clone(), source.clone()).main())
}

pub fn from_path(topology: &Topology, path_to_config: impl AsRef<Path>) -> Blueprint {
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
        let mut validated_configs = false;
        let mut first_envelope = match self.ctx.recv().await {
            Some(e) => {
                msg!(match e {
                    // We do not expect the first message to be `ConfigUpdated` because when the
                    // actor is first started, `UpdateConfig` is consumed by the supervisor.
                    (StartEntrypoint { is_check_only, .. }, token) => {
                        fn convert_to_protocol(
                            errors: Vec<ReloadConfigsError>,
                        ) -> Vec<EntrypointError> {
                            errors
                                .into_iter()
                                .map(|e| EntrypointError::new(e.group, e.reason))
                                .collect()
                        }

                        if is_check_only {
                            match self.load_and_check_configs().await {
                                Ok(_) => self.ctx.respond(token, Ok(())),
                                Err(errors) => self.ctx.respond(
                                    token,
                                    Err(StartEntrypointRejected::new(convert_to_protocol(errors))),
                                ),
                            }
                            return;
                        } else {
                            match self.load_and_update_configs(true).await {
                                Ok(_) => self.ctx.respond(token, Ok(())),
                                Err(errors) => {
                                    self.ctx.respond(
                                        token,
                                        Err(StartEntrypointRejected::new(convert_to_protocol(
                                            errors,
                                        ))),
                                    );
                                    panic!("configs are invalid at startup");
                                }
                            }
                        }
                        validated_configs = true;
                        None
                    }
                    // In case the actor was restarted by the supervisor, no `StartEntrypoint`
                    // message is sent and we need to process any incoming envelopes normally.
                    e => Some(e),
                })
            }
            // We treat `None` as a symptom of termination and stop the actor straightaway.
            None => return,
        };

        // Reload and validate configs in case the actor was restarted by the
        // supervisor.
        if !validated_configs && self.load_and_update_configs(true).await.is_ok() {
            panic!("configs are invalid at startup");
        }

        let signal = Signal::new(SignalKind::UnixHangup, ReloadConfigs::default());
        self.ctx.attach(signal);
        let signal = Signal::new(SignalKind::UnixUser2, ReloadConfigs::forcing());
        self.ctx.attach(signal);

        while let Some(envelope) = match first_envelope.take() {
            e @ Some(..) => e,
            None => self.ctx.recv().await,
        } {
            msg!(match envelope {
                (ReloadConfigs { force }, token) => {
                    let response = self
                        .load_and_update_configs(force)
                        .await
                        .map_err(|errors| ReloadConfigsRejected { errors });

                    self.ctx.respond(token, response);
                }
            })
        }
    }

    async fn load_configs(&self) -> Result<Value, Vec<ReloadConfigsError>> {
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
                return Err(vec![ReloadConfigsError {
                    group: scope::meta().group.clone(),
                    reason: error,
                }]);
            }
        };

        Deserialize::deserialize(config).map_err(|error| {
            error!(%error, "invalid config");
            vec![ReloadConfigsError {
                group: scope::meta().group.clone(),
                reason: error.to_string(),
            }]
        })
    }

    async fn load_and_check_configs(&self) -> Result<(), Vec<ReloadConfigsError>> {
        let configs = self.load_configs().await?;

        // Here we rely on the fact that the first `ValidateConfig` message is consumed
        // by the supervisor and no actors are actually started.
        let configs = match_configs(&self.topology, &configs);
        self.validate_all(&configs).await
    }

    async fn load_and_update_configs(
        &mut self,
        force: bool,
    ) -> Result<(), Vec<ReloadConfigsError>> {
        let configs = self.load_configs().await?;

        let mut configs = match_configs(&self.topology, &configs);

        // Filter out up-to-date configs if needed.
        if !force {
            configs.retain(|c| {
                self.versions
                    .get(&c.group_name)
                    .map_or(true, |v| c.hash != *v)
            });
        }

        if configs.is_empty() {
            info!("all groups' configs are up-to-date, nothing to update");
            return Ok(());
        }

        // Validation.
        // It is important that we perform config validation of *all* groups before
        // starting any actors. This ensures that each actor has a config to work with.
        // The fact that no actors are started here is guaranteed by the supervisor,
        // which consumes the first `ValidateConfig` it receives.
        let status = ActorStatus::NORMAL.with_details("validating");
        self.ctx.set_status(status);

        if let Err(errors) = self.validate_all(&configs).await {
            error!("config validation failed");
            self.ctx.set_status(ActorStatus::NORMAL);
            return Err(errors);
        }

        // Updating.
        let status = ActorStatus::NORMAL.with_details("updating");
        self.ctx.set_status(status);
        self.update_all(&configs).await;

        self.ctx.set_status(ActorStatus::NORMAL);

        // Update versions.
        let updated_groups: Vec<String> = configs
            .into_iter()
            .inspect(|config| {
                self.versions.insert(config.group_name.clone(), config.hash);
            })
            .map(|config| config.group_name)
            .collect();

        info!(
            message = "groups' configs are updated",
            groups = ?updated_groups,
        );

        Ok(())
    }

    async fn validate_all(
        &self,
        configs: &[ConfigWithMeta],
    ) -> Result<(), Vec<ReloadConfigsError>> {
        let futures = configs
            .iter()
            .cloned()
            .map(|item| {
                let group = item.group_name;
                let fut = self
                    .ctx
                    .request_to(item.addr, ValidateConfig::new(item.config))
                    .all()
                    .resolve();

                wrap_long_running_future(
                    fut,
                    group,
                    "group is validing the config suspiciously long",
                )
            })
            .collect::<Vec<_>>();

        let errors = future::join_all(futures)
            .await
            .into_iter()
            .flat_map(|(group, results)| results.into_iter().map(move |res| (group.clone(), res)))
            .filter_map(|(group, result)| match result {
                // NOTE: Since actors discard `ValidateConfig` by default, it is ok to receive
                // `Err(RequestError::Closed(..))` here.
                Ok(Ok(_)) | Err(_) => None,
                Ok(Err(reject)) => Some((group, reject.reason)),
            })
            // TODO: include actor keys in the error message.
            .inspect(|(group, reason)| error!(%group, %reason, "invalid config"))
            .map(|(group, reason)| ReloadConfigsError { group, reason })
            .collect::<Vec<_>>();

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    async fn update_all(&self, configs: &[ConfigWithMeta]) {
        let futures = configs
            .iter()
            .cloned()
            .map(|item| {
                let group = item.group_name;
                // While `UpdateConfig` is defined as a request to cover more use cases, default
                // configurer simply sends out new configs instead of waiting for all groups to
                // process the message and respond. This is done for performance reasons. If an
                // actor has a congested mailbox or spends a lot of time processing each
                // message, updating configs using requests can take a lot of time.
                let fut = self.ctx.send_to(item.addr, UpdateConfig::new(item.config));

                wrap_long_running_future(
                    fut,
                    group,
                    "some actors in the group have congested mailboxes, config update is stalled",
                )
            })
            .collect::<Vec<_>>();

        future::join_all(futures).await;
    }
}

async fn wrap_long_running_future<F: Future>(
    f: F,
    group_name: String,
    message: &'static str,
) -> (String, F::Output) {
    tokio::pin!(f);
    loop {
        select! {
            output = &mut f => return (group_name, output),
            _ = time::sleep(WARN_INTERVAL) => {
                warn!(group = %group_name, message);
            }
        }
    }
}

#[allow(dead_code)]
async fn ping(ctx: &Context, config_list: &[ConfigWithMeta]) -> bool {
    let futures = config_list
        .iter()
        .cloned()
        .map(|item| ctx.request_to(item.addr, Ping::default()).all().resolve())
        .collect::<Vec<_>>();

    // TODO: use `try_join_all`.
    let errors = future::join_all(futures)
        .await
        .into_iter()
        .flatten()
        .filter_map(|result| match result {
            Ok(()) | Err(RequestError::Ignored) => None,
            Err(RequestError::Failed) => Some(String::from("some group is closed")),
        })
        // TODO: include actor keys in the error message.
        .inspect(|reason| error!(%reason, "ping failed"));

    errors.count() == 0
}

async fn load_raw_config(path: impl AsRef<Path>) -> Result<Value, String> {
    let content = fs::read_to_string(path)
        .await
        .map_err(|err| err.to_string())?;
    toml::from_str(&content).map_err(|err| err.to_string())
}

fn match_configs(topology: &Topology, config: &Value) -> Vec<ConfigWithMeta> {
    let mut configs: Vec<ConfigWithMeta> = topology
        .locals()
        // Entrypoints' configs are updated only at startup.
        .filter(|group| !group.is_entrypoint)
        .map(|group| {
            let empty = Value::Map(Default::default());
            let common = helpers::lookup_value(config, "common").unwrap_or(&empty);
            let group_config = helpers::lookup_value(config, &group.name).cloned();
            let group_config = helpers::add_defaults(group_config, common);

            ConfigWithMeta {
                group_name: group.name.clone(),
                addr: group.addr,
                hash: fxhash::hash64(&group_config),
                config: AnyConfig::from_value(group_config),
            }
        })
        .collect();
    // Config parsing happens in the supervisor, which is executed in this actor
    // when it performs `send_to(addr, UpdateConfig)`. User actor groups can
    // have arbitrary large configs, taking a considerable time to deserialize.
    // System actors, on the other hand, have tiny configs, so we give them a
    // priority to apply system-wide settings (such as logging level) faster.
    configs.sort_by_key(|config| !config.group_name.starts_with("system."));
    configs
}
