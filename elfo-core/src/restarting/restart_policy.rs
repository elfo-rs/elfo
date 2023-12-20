use std::{num::NonZeroU64, time::Duration};

use tracing::warn;

use crate::ActorStatus;

/// The behaviour on actor termination.
#[derive(Debug, Clone, PartialEq)]
pub struct RestartPolicy {
    pub(crate) mode: RestartMode,
}

impl Default for RestartPolicy {
    fn default() -> Self {
        Self::never()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum RestartMode {
    Always(RestartParams),
    OnFailure(RestartParams),
    Never,
}

impl RestartPolicy {
    pub fn always(restart_params: RestartParams) -> Self {
        Self {
            mode: RestartMode::Always(restart_params),
        }
    }

    pub fn on_failure(restart_params: RestartParams) -> Self {
        Self {
            mode: RestartMode::OnFailure(restart_params),
        }
    }

    pub fn never() -> Self {
        Self {
            mode: RestartMode::Never,
        }
    }

    pub(crate) fn restarting_allowed(&self, status: &ActorStatus) -> bool {
        match &self.mode {
            RestartMode::Always(_) => true,
            RestartMode::OnFailure(_) => status.is_failed(),
            _ => false,
        }
    }

    pub(crate) fn restart_params(&self) -> Option<RestartParams> {
        match &self.mode {
            RestartMode::Always(params) | RestartMode::OnFailure(params) => Some(*params),
            _ => None,
        }
    }
}

/// Restart parameters for the backoff strategy when an actor restarts based on
/// the [RestartPolicy].
#[derive(Debug, Copy, Clone, PartialEq)]
pub struct RestartParams {
    pub(crate) min_backoff: Duration,
    pub(crate) max_backoff: Duration,
    pub(crate) auto_reset: Duration,
    pub(crate) max_retries: NonZeroU64,
    pub(crate) factor: f64,
}

impl RestartParams {
    /// Creates a new instance with the specified minimum and maximum backoff
    /// durations. The default values for `auto_reset`, `max_retries`, and
    /// `factor` are set as follows:
    /// - `auto_reset = min_backoff`
    /// - `max_retries = NonZeroU64::MAX`
    /// - `factor = 2.0`
    pub fn new(min_backoff: Duration, max_backoff: Duration) -> Self {
        RestartParams {
            min_backoff,
            max_backoff: min_backoff.max(max_backoff),
            auto_reset: min_backoff,
            max_retries: NonZeroU64::MAX,
            factor: 2.0,
        }
    }

    /// Sets the duration deemed sufficient to consider an actor healthy. Once
    /// this duration elapses, the backoff strategy automatically resets,
    /// including retry counting, effectively treating the next attempt as the
    /// first retry. Therefore, setting the `auto_reset` to small values,
    /// such as [Duration::ZERO], can result in the absence of a limit on
    /// the maximum number of retries. After the backoff strategy resets, the
    /// actor will restart immediately.
    ///
    /// `None` does not change the `auto_reset` setting.
    ///
    /// If the function isn't used, `auto_reset = min_backoff` is used by
    /// default.
    pub fn auto_reset(self, auto_reset: impl Into<Option<Duration>>) -> Self {
        Self {
            auto_reset: auto_reset.into().unwrap_or(self.auto_reset),
            ..self
        }
    }

    /// Sets the factor used to calculate the next backoff duration.
    /// The factor should be a finite value and should not be negative;
    /// otherwise, a warning will be emitted.
    ///
    /// `None` value does not change the `factor` setting.
    ///
    /// If the function isn't used, `factor = 2.0` is used by default.
    pub fn factor(self, factor: impl Into<Option<f64>>) -> Self {
        let factor = factor.into().unwrap_or(self.factor);
        let factor = if !factor.is_finite() || factor.is_sign_negative() {
            warn!("factor should be a finite value and should not be negative");
            0.0
        } else {
            factor
        };
        Self { factor, ..self }
    }

    /// Sets the maximum number of allowed retries. Each time the actor
    /// restarts, it counts as a retry. If the retries reach the specified
    /// max_retries, the actor stops restarting. If the actor lives long
    /// enough to be considered healthy (see [RestartParams::auto_reset]), the
    /// restart count goes back to zero, and the next restart is considered
    /// the first retry again.
    ///
    /// `None` does not change the `max_retries` setting.
    ///
    /// If the function isn't used, `max_retries = NonZeroU64::MAX` is used by
    /// default.
    pub fn max_retries(self, max_retries: impl Into<Option<NonZeroU64>>) -> Self {
        Self {
            max_retries: max_retries.into().unwrap_or(self.max_retries),
            ..self
        }
    }
}
