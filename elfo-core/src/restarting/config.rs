//! [Config].
//!
//! [Config]: RestartPolicyConfig

use std::{num::NonZeroU64, time::Duration};

use serde::Deserialize;

use crate::restarting::restart_policy::{RestartParams, RestartPolicy};

/// Restart policy configuration, `Never` by default.
///
/// Overrides [`ActorGroup::restart_policy()`].
/// Can be overriden by actor using [`Context::set_restart_policy()`].
///
/// See [The Actoromicon] for details.
///
/// # Example
/// ```toml
/// [some_group]
/// system.restart_policy.when = "OnFailure"
/// system.restart_policy.min_backoff = "5s"
/// system.restart_policy.max_backoff = "30s"
/// ```
///
/// [`ActorGroup::restart_policy()`]: crate::ActorGroup::restart_policy
/// [`Context::set_restart_policy()`]: crate::Context::set_restart_policy
/// [The Actoromicon]: https://actoromicon.rs/ch04-02-supervision.html#restart
#[derive(Debug, Clone, Default, Deserialize)]
pub struct RestartPolicyConfig(pub Option<WhenConfig>);

/// Restart policies.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "when")]
pub enum WhenConfig {
    /// Restart both on failures and terminations.
    Always(RestartParamsConfig),
    /// Restart only on failures.
    OnFailure(RestartParamsConfig),
    /// Never restart actors.
    Never,
}

/// Restart policy parameters.
#[derive(Debug, Clone, Deserialize)]
pub struct RestartParamsConfig {
    /// Minimal restart time limit.
    #[serde(with = "humantime_serde")]
    min_backoff: Duration,
    /// Maximum restart time limit.
    #[serde(with = "humantime_serde")]
    max_backoff: Duration,
    /// The duration of an actor's lifecycle sufficient to deem the actor
    /// healthy.
    ///
    /// The default value is `min_backoff`.
    #[serde(with = "humantime_serde", default)]
    auto_reset: Option<Duration>,
    /// The limit on retry attempts, after which the actor stops attempts to
    /// restart.
    ///
    /// Unlimited by default.
    max_retries: Option<NonZeroU64>,
    /// The value to multiply the current delay with for each retry attempt.
    ///
    /// Default value is 2.0.
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
