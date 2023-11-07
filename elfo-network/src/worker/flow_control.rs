//! Implements data flow control for connections to remote actor groups.
//!
//! The basic idea is that we want to adjust the amount of traffic local actor
//! sends to a remote actor according to it's processing rate. This is important
//! for two reasons:
//!  1. Avoid overwhelming slow remote actor with lots of messages from a fast
//!     local actor
//!  2. Avoid network congestion
//!
//! This is implemented through maintaining two "windows": one for the sender
//! and another one for the receiver. Window is an estimate of how many messages
//! the receiver can handle during some period of time, implemented as a
//! semaphore. Before sending a message, sender acquires budget from the window.
//! When the receiver acknowledges that a message was processed, budget is
//! returned to the window.
//!
//! Finally, there are two ways the sender can send a message: bounded and
//! unbounded. Bounded send waits until there is enough budget in the sender's
//! window before sending the message. Unbounded send always gets the budget
//! immediately, even if there is zero available. This can lead to window size
//! being a negative number.
//!
//! See `TxFlowControl` and `RxFlowControl` for implementations of sender's and
//! receiver's windows respectively.
//!
//! If none of this makes sense, see https://en.wikipedia.org/wiki/Flow_control_(data) and
//! https://en.wikipedia.org/wiki/Sliding_window_protocol for a more elaborate explanation.

use std::sync::atomic::{AtomicI32, Ordering};

// We don't want to send window updates for tiny changes, but instead
// aggregate them when the changes are significant.
//
// To avoid float arithmetic, the "ratio" is represented by the numerator and
// denominator. For example, a 50% ratio is simply represented as 1/2.
const UNCLAIMED_NUMERATOR: i32 = 1;
const UNCLAIMED_DENOMINATOR: i32 = 2;

const_assert!(0 <= UNCLAIMED_NUMERATOR && UNCLAIMED_NUMERATOR < UNCLAIMED_DENOMINATOR);

/// Determines the window of sender's side.
/// Used by multiple threads simultaneously.
/// The window can be negative (e.g. due to `unbounded_send()`),
#[derive(Debug)]
pub(super) struct TxFlowControl(AtomicI32);

impl TxFlowControl {
    /// Creates a new controller with the given initial window.
    /// The initial window should be got from the receiver.
    pub(super) fn new(initial: i32) -> Self {
        assert!(initial > 0, "negative initial window");
        Self(AtomicI32::new(initial))
    }

    /// Tries to decrease the window by 1.
    /// Called when a message is considered to be sent boundedly.
    ///
    /// Returns `true` if the window is decreased.
    pub(super) fn try_acquire(&self) -> bool {
        let prev = self.0.fetch_sub(1, Ordering::Relaxed);
        assert_ne!(prev, i32::MIN, "window underflow");

        if prev > 0 {
            true
        } else {
            self.0.fetch_add(1, Ordering::Relaxed);
            false
        }
    }

    /// Decreases the window by 1.
    /// Called when a message is considered to be sent unboundedly.
    pub(super) fn do_acquire(&self) {
        let prev = self.0.fetch_sub(1, Ordering::Relaxed);
        assert_ne!(prev, i32::MIN, "window underflow");
    }

    /// Changes the window by `delta` returned from `RxFlowControl::release()`.
    ///
    /// Returns `true` if the window is positive.
    pub(super) fn release(&self, delta: i32) -> bool {
        let prev = self.0.fetch_add(delta, Ordering::Relaxed);
        prev.checked_add(delta).expect("window overflow") > 0
    }
}

/// Determines the window of receiver's side.
/// The window can be negative (e.g. due to `unbounded_send()`),
#[derive(Debug)]
pub(super) struct RxFlowControl {
    /// Window the sender knows about.
    tx_window: i32,
    /// Window that should be advertised to the sender.
    rx_window: i32,
}

impl RxFlowControl {
    /// Creates a new controller with the given initial window.
    pub(super) fn new(initial: i32) -> Self {
        assert!(initial >= 0, "initial window must be non-negative");
        Self {
            tx_window: initial,
            rx_window: initial,
        }
    }

    /// Decreases the window by 1. Called when a message is received.
    /// `tx_knows = true` means the sender knew the address of an actor and
    /// decreased its window, otherwise it used the routing subsystem.
    pub(super) fn do_acquire(&mut self, tx_knows: bool) {
        if tx_knows {
            self.tx_window = self.tx_window.checked_sub(1).expect("window underflow");
        }
        self.rx_window = self.rx_window.checked_sub(1).expect("window underflow");
    }

    // Increases the window by `delta` (e.g. 1 when a message is sent to an actor).
    // Returns `Some(delta)` if the window update should be sent to the sender.
    pub(super) fn release(&mut self, delta: i32) -> Option<i32> {
        self.rx_window = self.rx_window.checked_add(delta).expect("window overflow");

        if self.rx_window <= 0 {
            return None;
        }

        if self.tx_window >= self.rx_window {
            // TODO: send a negative window delta if the difference is too big.
            return None;
        }

        // We don't want to send window updates for tiny changes, but instead
        // aggregate them when the changes are significant.
        let threshold = self.tx_window / UNCLAIMED_DENOMINATOR * UNCLAIMED_NUMERATOR;
        let release = self.rx_window.saturating_sub(self.tx_window);

        if release >= threshold {
            self.tx_window += release;
            Some(release)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tx_flow_control() {
        let fc = TxFlowControl::new(2);
        assert!(fc.try_acquire());
        assert!(fc.try_acquire());
        assert!(!fc.try_acquire());
        assert!(!fc.try_acquire());
        assert!(fc.release(1));
        assert!(fc.try_acquire());
        assert!(!fc.try_acquire());
        assert!(fc.release(2));
        assert!(fc.release(1));

        for _ in 0..5 {
            fc.do_acquire();
        }
        assert!(!fc.release(1));
        assert!(!fc.release(1));
        assert!(fc.release(1));
    }

    // Checks that the number of sent window updates is small.
    #[test]
    fn rx_flow_control() {
        // Real-time case: a system is stable, incoming rate = processing rate.
        let mut fc = RxFlowControl::new(1000);
        let total = 1000;
        let sent = (0..total).fold(0, |sent, _| {
            fc.do_acquire(true);
            sent + u32::from(fc.release(1).is_some())
        });
        let ratio = sent as f64 / total as f64;
        assert!(ratio < 0.01, "{}", ratio);

        // Resuming after an actor is unstuck.
        let mut fc = RxFlowControl::new(0);
        let total = 2000;
        let sent = (0..total).fold(0, |sent, _| sent + fc.release(1).is_some() as u32);
        let ratio = sent as f64 / total as f64;
        assert!(ratio < 0.01, "{}", ratio);
    }
}
