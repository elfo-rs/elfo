#![warn(rust_2018_idioms, unreachable_pub)]

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
use elfo_macros::{message, msg_raw as msg};

use elfo::{
    config::AnyConfig,
    errors::RequestError,
    messages::{ConfigRejected, Ping, UpdateConfig, ValidateConfig},
    signal::{Signal, SignalKind},
    ActorGroup, ActorStatus, Addr, Context, Request, Schema, Topology,
};

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
}

#[message(elfo = elfo_core)]
pub struct ReloadConfigs;

struct Configurer {
    ctx: Context,
    topology: Topology,
    source: ConfigSource,
}

impl Configurer {
    fn new(ctx: Context, topology: Topology, source: ConfigSource) -> Self {
        Self {
            ctx,
            topology,
            source,
        }
    }

    async fn main(self) {
        let signal = Signal::new(SignalKind::Hangup, || ReloadConfigs);
        let mut ctx = self.ctx.clone().with(&signal);
        let can_start = self.load_and_update_configs().await;

        while let Some(envelope) = ctx.recv().await {
            msg!(match envelope {
                (Ping, token) if can_start => ctx.respond(token, ()),
                (Ping, token) => drop(token),
                ReloadConfigs => {
                    self.load_and_update_configs().await;
                }
            })
        }
    }

    async fn load_and_update_configs(&self) -> bool {
        let config = match &self.source {
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

        let system_updated = self.update_configs(&config, TopologyFilter::System).await;
        let user_udpated = self.update_configs(&config, TopologyFilter::User).await;
        system_updated && user_udpated
    }

    async fn update_configs(&self, config: &Value, filter: TopologyFilter) -> bool {
        let config_list = match_configs(&self.topology, config.clone(), filter);

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
                    .request(make_msg(item.config))
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

fn match_configs(
    topology: &Topology,
    config: Value,
    filter: TopologyFilter,
) -> Vec<ConfigWithMeta> {
    let configs: FxHashMap<String, Value> = Deserialize::deserialize(config).unwrap_or_default();

    topology
        .actor_groups()
        // Entrypoints' configs are updated only at startup.
        .filter(|group| !group.is_entrypoint)
        .filter(|group| match filter {
            TopologyFilter::System => group.name.starts_with("system."),
            TopologyFilter::User => !group.name.starts_with("system."),
        })
        .map(|group| ConfigWithMeta {
            group_name: group.name.clone(),
            addr: group.addr,
            config: get_config(&configs, &group.name)
                .map_or_else(AnyConfig::default, AnyConfig::new),
        })
        .collect()
}

fn get_config(configs: &FxHashMap<String, Value>, path: &str) -> Option<Value> {
    let mut parts_iter = path.split('.');
    let mut node = configs.get(parts_iter.next()?)?;
    for part in parts_iter {
        node = if let Value::Map(map) = node {
            map.get(&Value::String(part.to_owned()))?
        } else {
            return None;
        };
    }
    Some(node.clone())
}

#[cfg(test)]
mod test {
    use super::*;

    use std::collections::BTreeMap;

    #[test]
    fn get_config_should_get_config_by_key() {
        assert_eq!(get_config(&create_configs(), "alpha"), Some(alpha_value()));
    }

    #[test]
    fn get_config_should_get_default_for_missing_key() {
        assert_eq!(get_config(&create_configs(), "beta"), None);
    }

    #[test]
    fn get_config_should_get_default_for_completely_missing_path() {
        assert_eq!(get_config(&create_configs(), "beta.beta.gamma"), None);
    }

    #[test]
    fn get_config_should_get_default_for_partially_missing_path() {
        assert_eq!(get_config(&create_configs(), "gamma.zeta.beta"), None);
        assert_eq!(get_config(&create_configs(), "alpha.zeta.beta"), None);
        assert_eq!(get_config(&create_configs(), "gamma.zeta.theta.beta"), None);
    }

    #[test]
    fn get_config_should_get_config_by_path() {
        assert_eq!(
            get_config(&create_configs(), "gamma.zeta.theta"),
            Some(theta_value())
        );
    }

    /// ```json
    /// {
    ///     "alpha": "beta",
    ///     "gamma": {
    ///         "zeta": { "theta": "iota" }
    ///     }
    /// }
    /// ```
    fn create_configs() -> FxHashMap<String, Value> {
        let mut zeta_value: BTreeMap<Value, Value> = Default::default();
        zeta_value.insert(Value::String("theta".to_owned()), theta_value());
        let zeta_value = Value::Map(zeta_value);
        let mut gamma_value: BTreeMap<Value, Value> = Default::default();
        gamma_value.insert(Value::String("zeta".to_owned()), zeta_value);
        let gamma_value = Value::Map(gamma_value);
        let mut configs: FxHashMap<String, Value> = Default::default();
        configs.insert("alpha".to_owned(), alpha_value());
        configs.insert("gamma".to_owned(), gamma_value);
        configs
    }

    fn alpha_value() -> Value {
        Value::String("beta".to_owned())
    }

    fn theta_value() -> Value {
        Value::String("iota".to_owned())
    }
}
