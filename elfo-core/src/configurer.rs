use std::{
    fs,
    path::{Path, PathBuf},
};

use futures::future;
use fxhash::FxHashMap;
use serde::{de::Deserializer, Deserialize};
use serde_value::Value;
use tracing::{error, info};

use elfo_macros::msg_internal as msg;

use crate::{
    addr::Addr,
    config::AnyConfig,
    context::Context,
    errors::RequestError,
    group::{ActorGroup, Schema},
    messages::{ConfigRejected, ConfigUpdated, UpdateConfig},
    topology::Topology,
};

pub fn fixture(topology: &Topology, config: impl for<'de> Deserializer<'de>) -> Schema {
    let config = Value::deserialize(config).map_err(|err| err.to_string());
    let source = ConfigSource::Fixture(config);
    let topology = topology.clone();
    ActorGroup::new().exec(move |ctx| configurer(ctx, topology.clone(), source.clone()))
}

pub fn from_path(topology: &Topology, path_to_config: impl AsRef<Path>) -> Schema {
    let source = ConfigSource::File(path_to_config.as_ref().to_path_buf());
    let topology = topology.clone();
    ActorGroup::new().exec(move |ctx| configurer(ctx, topology.clone(), source.clone()))
}

#[derive(Clone)]
enum ConfigSource {
    File(PathBuf),
    Fixture(Result<Value, String>),
}

async fn configurer(mut ctx: Context, topology: Topology, source: ConfigSource) {
    let mut signal = Signal::new();
    let mut request = ctx.recv().await;

    loop {
        if request.is_none() {
            signal.recv().await;
        }

        let just_started = request.is_some();
        let is_ok = update_configs(&ctx, &topology, &source, just_started).await;

        if let Some(request) = request.take() {
            msg!(match request {
                (UpdateConfig { .. }, token) if is_ok => ctx.respond(token, Ok(ConfigUpdated {})),
                (UpdateConfig { .. }, token) => ctx.respond(
                    token,
                    Err(ConfigRejected {
                        reason: "invalid configuration".into(),
                    })
                ),
            })
        }
    }
}

async fn update_configs(
    ctx: &Context,
    topology: &Topology,
    source: &ConfigSource,
    skip_validation: bool,
) -> bool {
    let config = match &source {
        ConfigSource::File(path) => load_raw_config(path),
        ConfigSource::Fixture(value) => value.clone(),
    };

    let config = match config {
        Ok(config) => config,
        Err(reason) => {
            error!(%reason, "invalid config");
            return false;
        }
    };

    let config_list = match_configs(&topology, config);

    if !skip_validation {
        info!("config validation");
        // TODO: send `ValidateConfig`.
    }

    info!("sending config updates");

    let futures = config_list
        .iter()
        .cloned()
        .map(|(addr, config)| {
            ctx.request(UpdateConfig { config })
                .from(addr)
                .all()
                .resolve()
        })
        .collect::<Vec<_>>();

    let errors = future::join_all(futures)
        .await
        .into_iter()
        .flatten()
        .filter_map(|result| match result {
            Ok(Ok(_)) | Err(RequestError::Ignored) => None,
            Ok(Err(reject)) => Some(reject.reason),
            Err(RequestError::Closed(_)) => Some("some group is closed".into()),
        })
        // TODO: provide more info.
        .inspect(|reason| error!(%reason, "invalid config"));

    errors.count() == 0
}

fn load_raw_config(path: impl AsRef<Path>) -> Result<Value, String> {
    let content = fs::read_to_string(path).map_err(|err| err.to_string())?;
    toml::from_str(&content).map_err(|err| err.to_string())
}

fn match_configs(topology: &Topology, config: Value) -> Vec<(Addr, AnyConfig)> {
    let configs: FxHashMap<String, Value> = Deserialize::deserialize(config).unwrap_or_default();

    topology
        .actor_groups()
        // Entrypoints' configs are updated only at startup.
        .filter(|group| !group.is_entrypoint)
        .map(|group| {
            // TODO: handle "a.b.c" cases more carefully.
            let group_config = configs
                .get(&group.name)
                .cloned()
                .map_or_else(AnyConfig::default, AnyConfig::new);

            (group.addr, group_config)
        })
        .collect()
}

// TODO: reimpl `signal` using `Source` trait.
// TODO: handle SIGHUP (unix only).
struct Signal {}

impl Signal {
    fn new() -> Self {
        Self {}
    }

    async fn recv(&mut self) {
        let () = future::pending().await;
    }
}
