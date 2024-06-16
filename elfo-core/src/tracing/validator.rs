use std::{convert::TryFrom, time::Duration};

use super::trace_id::{TraceId, TruncatedTime};
use crate::node;

/// A [`TraceId`] validator.
///
/// By default, it checks the following properties:
/// * Cannot be zero.
/// * Must be 63-bit.
/// * Cannot have the same node no, because it's sent outside the elfo system.
///
/// Optionally, it can also check the time difference, see
/// [`TraceIdValidator::max_time_difference`].
#[derive(Clone, Default)]
pub struct TraceIdValidator {
    max_time_difference: Option<Duration>,
}

// Errors
const CANNOT_BE_ZERO: &str = "cannot be zero";
const HIGHEST_BIT_MUST_BE_ZERO: &str = "highest bit must be zero";
const INVALID_NODE_NO: &str = "invalid node no";
const INVALID_TIMESTAMP: &str = "invalid timestamp";

impl TraceIdValidator {
    /// Allowed time difference between now and timestamp in a raw trace id.
    /// Checks the absolute difference to handle both situations:
    /// * Too old timestamp, possible incorrectly generated.
    /// * Too new timestamp, something wrong with time synchronization.
    pub fn max_time_difference(&mut self, time_lag: impl Into<Option<Duration>>) -> Self {
        self.max_time_difference = time_lag.into();
        self.clone()
    }

    /// Validates a raw trace id transforms it into [`TraceId`] if valid.
    pub fn validate(&self, raw_trace_id: u64) -> Result<TraceId, &'static str> {
        let trace_id = TraceId::try_from(raw_trace_id).map_err(|_| CANNOT_BE_ZERO)?;
        let layout = trace_id.to_layout();

        // The highest bit must be zero.
        if raw_trace_id & (1 << 63) != 0 {
            return Err(HIGHEST_BIT_MUST_BE_ZERO);
        }

        // We don't allow to specify valid `node_no` for now,
        // but at least we can check that it isn't this node.
        if layout.node_no == node::node_no() {
            return Err(INVALID_NODE_NO);
        }

        if let Some(time_lag) = self.max_time_difference {
            let truncated_now = TruncatedTime::now();
            let delta = truncated_now.abs_delta(layout.timestamp);

            if i64::from(delta) > time_lag.as_secs() as i64 {
                return Err(INVALID_TIMESTAMP);
            }
        }

        Ok(trace_id)
    }
}

#[test]
fn it_works() {
    node::set_node_no(65535);

    elfo_utils::time::with_mock(|mock| {
        let validator = TraceIdValidator::default().max_time_difference(Duration::from_secs(5));

        assert_eq!(validator.validate(0), Err(CANNOT_BE_ZERO));
        assert_eq!(validator.validate(1 << 63), Err(HIGHEST_BIT_MUST_BE_ZERO));
        assert_eq!(
            validator.validate(u64::from(node::node_no().map_or(0, |n| n.into_bits())) << 22),
            Err(INVALID_NODE_NO)
        );

        assert!(validator.validate(5 << 38).is_ok());
        assert!(validator.validate(((1 << 25) - 5) << 38).is_ok());
        assert_eq!(validator.validate(6 << 38), Err(INVALID_TIMESTAMP));
        assert_eq!(
            validator.validate(((1 << 25) - 6) << 38),
            Err(INVALID_TIMESTAMP)
        );

        mock.advance(Duration::from_secs(10));
        assert_eq!(validator.validate(16 << 38), Err(INVALID_TIMESTAMP));
        assert_eq!(validator.validate(4 << 38), Err(INVALID_TIMESTAMP));
    });
}
