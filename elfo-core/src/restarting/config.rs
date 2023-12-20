use std::{num::NonZeroU64, time::Duration};

use serde::Deserialize;

use crate::restarting::restart_policy::{RestartParams, RestartPolicy};

#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct RestartPolicyConfig(Option<WhenConfig>);

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "when")]
enum WhenConfig {
    Always(RestartParamsConfig),
    OnFailure(RestartParamsConfig),
    Never,
}

#[derive(Debug, Clone, Deserialize)]
struct RestartParamsConfig {
    #[serde(with = "humantime_serde")]
    min_backoff: Duration,
    #[serde(with = "humantime_serde")]
    max_backoff: Duration,
    #[serde(with = "humantime_serde", default)]
    auto_reset: Option<Duration>,
    max_retries: Option<NonZeroU64>,
    factor: Option<f64>,
}

impl RestartPolicyConfig {
    pub(crate) fn make_policy(&self) -> Option<RestartPolicy> {
        self.0.as_ref().map(|cfg| match cfg {
            WhenConfig::Always(rp_cfg) => RestartPolicy::always(rp_cfg.make_params()),
            WhenConfig::OnFailure(rp_cfg) => RestartPolicy::on_failure(rp_cfg.make_params()),
            WhenConfig::Never => RestartPolicy::never(),
        })
    }
}

impl RestartParamsConfig {
    fn make_params(&self) -> RestartParams {
        RestartParams::new(self.min_backoff, self.max_backoff)
            .factor(self.factor)
            .auto_reset(self.auto_reset)
            .max_retries(self.max_retries)
    }
}
