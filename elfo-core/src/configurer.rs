use std::{error::Error, fs, path::Path, sync::Arc};

use futures::future;
use fxhash::FxHashMap;
use serde_value::Value;
use tracing::{error, info};

use crate::{
    addr::Addr,
    config::AnyConfig,
    errors::RequestError,
    group::{ActorGroup, Schema},
    messages::UpdateConfig,
    topology::Topology,
};

pub fn configurers(
    topology: &Topology,
    path_to_config: impl AsRef<Path>,
) -> Result<Schema, Box<dyn Error>> {
    let config_list = Arc::new(load_configs(topology, path_to_config)?);

    let schema = ActorGroup::new().exec(move |ctx| {
        let config_list = config_list.clone();
        let mut signal = Signal::new();
        let mut just_started = true;

        async move {
            loop {
                signal.recv().await;

                if !just_started {
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
                    .inspect(|reason| error!(%reason, "invalid config"))
                    .collect::<Vec<_>>();

                if !errors.is_empty() && just_started {
                    // TODO: use some API for it?
                    std::process::exit(1);
                }

                just_started = false;
            }
        }
    });

    Ok(schema)
}

fn load_configs(
    topology: &Topology,
    path: impl AsRef<Path>,
) -> Result<Vec<(Addr, AnyConfig)>, Box<dyn Error>> {
    let content = fs::read_to_string(path)?;
    let config: FxHashMap<String, Value> = toml::from_str(&content)?;

    Ok(topology
        .actor_groups()
        .filter(|group| !group.name.contains("configurer")) // XXX: find another way.
        .map(|group| {
            // TODO: handle "a.b.c" cases more carefully.
            let group_config = config
                .get(&group.name)
                .cloned()
                .map_or_else(AnyConfig::default, AnyConfig::new);

            (group.addr, group_config)
        })
        .collect())
}

// TODO: handle SIGHUP (unix only).
struct Signal {
    is_fired: bool,
}

impl Signal {
    fn new() -> Self {
        Self { is_fired: false }
    }

    async fn recv(&mut self) {
        if self.is_fired {
            let () = future::pending().await;
        }

        self.is_fired = true;
    }
}
