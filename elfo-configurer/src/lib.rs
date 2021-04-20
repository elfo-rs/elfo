use std::{
    fs,
    path::{Path, PathBuf},
};

use futures::{future, FutureExt};
use fxhash::FxHashMap;
use serde::{de::Deserializer, Deserialize};
use serde_value::Value;
use tracing::error;

use elfo_core as elfo;
use elfo_macros::msg_raw as msg;

use elfo::{
    config::AnyConfig,
    errors::RequestError,
    messages::{ConfigRejected, Ping, UpdateConfig, ValidateConfig},
    ActorGroup, ActorStatus, Addr, Context, Request, Schema, Topology,
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

#[derive(Clone)]
struct ConfigWithMeta {
    group_name: String,
    addr: Addr,
    config: AnyConfig,
}

async fn configurer(mut ctx: Context, topology: Topology, source: ConfigSource) {
    let mut signal = Signal::new();
    let mut request = ctx.recv().await;

    loop {
        if request.is_none() {
            signal.recv().await;
        }

        let is_ok = update_configs(&ctx, &topology, &source).await;

        if let Some(request) = request.take() {
            msg!(match request {
                (Ping, token) if is_ok => {
                    ctx.respond(token, ())
                }
                (Ping, token) => {
                    let _ = token;
                }
            })
        }
    }
}

async fn update_configs(ctx: &Context, topology: &Topology, source: &ConfigSource) -> bool {
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

    ctx.set_status(ActorStatus::NORMAL.with_details("config validation"));

    if !request_all(ctx, &config_list, |config| ValidateConfig { config }).await {
        error!("config validation failed");
        ctx.set_status(ActorStatus::NORMAL);
        return false;
    }

    ctx.set_status(ActorStatus::NORMAL.with_details("config updating"));

    if !request_all(ctx, &config_list, |config| UpdateConfig { config }).await {
        error!("config updating failed");
        ctx.set_status(ActorStatus::ALARMING.with_details("possibly incosistent configs"));
        return false;
    }

    if !ping(ctx, &config_list).await {
        error!("ping failed");
        ctx.set_status(ActorStatus::ALARMING.with_details("possibly incosistent configs"));
        return false;
    }

    ctx.set_status(ActorStatus::NORMAL);
    true
}

async fn request_all<R>(
    ctx: &Context,
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
            ctx.request(make_msg(item.config))
                .from(item.addr)
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
            Ok(Ok(_)) | Err(RequestError::Ignored) => None,
            Ok(Err(reject)) => Some((group, reject.reason)),
            Err(RequestError::Closed(_)) => Some((group, "some group is closed".into())),
        })
        // TODO: provide more info.
        .inspect(|(group, reason)| error!(%group, %reason, "invalid config"));

    errors.count() == 0
}

async fn ping(ctx: &Context, config_list: &[ConfigWithMeta]) -> bool {
    let futures = config_list
        .iter()
        .cloned()
        .map(|item| ctx.request(Ping).from(item.addr).all().resolve())
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

fn load_raw_config(path: impl AsRef<Path>) -> Result<Value, String> {
    let content = fs::read_to_string(path).map_err(|err| err.to_string())?;
    toml::from_str(&content).map_err(|err| err.to_string())
}

fn match_configs(topology: &Topology, config: Value) -> Vec<ConfigWithMeta> {
    let configs: FxHashMap<String, Value> = Deserialize::deserialize(config).unwrap_or_default();

    topology
        .actor_groups()
        // Entrypoints' configs are updated only at startup.
        .filter(|group| !group.is_entrypoint)
        .map(|group| {
            // TODO: handle "a.b.c" cases more carefully.
            let config = configs
                .get(&group.name)
                .cloned()
                .map_or_else(AnyConfig::default, AnyConfig::new);

            ConfigWithMeta {
                group_name: group.name.clone(),
                addr: group.addr,
                config,
            }
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
