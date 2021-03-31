use std::{
    fs,
    path::{Path, PathBuf},
};

use futures::future;
use fxhash::FxHashMap;
use serde::{de::Deserializer, Deserialize};
use serde_value::Value;
use tracing::{error, info};

use crate::{
    addr::Addr,
    config::AnyConfig,
    context::Context,
    errors::RequestError,
    group::{ActorGroup, Schema},
    messages::UpdateConfig,
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

async fn configurer(ctx: Context, topology: Topology, source: ConfigSource) {
    let mut signal = Signal::new();

    loop {
        let just_started = signal.recv().await;

        let is_ok = update_configs(&ctx, &topology, &source, just_started).await;

        if !is_ok && just_started {
            // TODO: provide API for this?
            std::process::exit(42);
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
    Ok(toml::from_str(&content).map_err(|err| err.to_string())?)
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

// TODO: handle SIGHUP (unix only).
struct Signal {
    just_started: bool,
}

impl Signal {
    fn new() -> Self {
        Self { just_started: true }
    }

    async fn recv(&mut self) -> bool {
        if !self.just_started {
            let () = future::pending().await;
        }

        let ret = self.just_started;
        self.just_started = false;
        ret
    }
}
