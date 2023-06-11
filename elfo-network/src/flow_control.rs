use derive_more::{Add, AddAssign, Sub, SubAssign};
use serde::{Deserialize, Serialize};

// We don't want to send window updates for tiny changes, but instead
// aggregate them when the changes are significant.
//
// To avoid float arithmetic, the "ratio" is represented by the numerator and
// denominator. For example, a 50% ratio is simply represented as 1/2.
const UNCLAIMED_NUMERATOR: i32 = 1;
const UNCLAIMED_DENOMINATOR: i32 = 2;

const_assert!(0 <= UNCLAIMED_NUMERATOR && UNCLAIMED_NUMERATOR < UNCLAIMED_DENOMINATOR);

/// Determines the window of sender's side.
/// Note that sender doesn't
#[derive(Debug)]
pub(crate) struct TxFlowControl(Window);

impl TxFlowControl {
    /// Creates a new controller with the given initial window.
    /// The initial window should be got from the receiver.
    pub(crate) fn new(initial: Window) -> Self {
        Self(initial)
    }

    /// Decreases the window by 1.
    /// Called when a message is considered to be sent.
    pub(crate) fn spend(&mut self) {
        self.0 -= Window(1);
    }

    /// Changes the window by `delta` returned from `RxFlowControl::release()`.
    /// Returns the number of permits that should be added to the semapthore.
    pub(crate) fn release(&mut self, delta: Window) -> usize {
        // The window can be negative (e.g. due to `unbounded_send()`),
        // but we cannot push a negative number to the semaphore.
        // So we need to adjust `delta` before converting it to `usize`.
        let debt = self.0.min(Window(0));
        let permits = (delta + debt).max(Window(0)).0 as usize;

        self.0 += delta;
        permits
    }
}

#[derive(Debug)]
pub(crate) struct RxFlowControl {
    /// Window the sender knows about.
    tx_window: Window,

    /// Window that should be advertised to the sender.
    rx_window: Window,
}

impl RxFlowControl {
    /// Creates a new controller with the given initial window.
    pub(crate) fn new(initial: Window) -> Self {
        Self {
            tx_window: initial,
            rx_window: initial,
        }
    }

    /// Decreases the window by 1.
    /// Called when a message is received.
    pub(crate) fn spend(&mut self) {
        self.tx_window -= Window(1);
        self.rx_window -= Window(1);
    }

    // Increases the window by 1 when a message is sent to an actor.
    // Returns `Some(delta)` if the window update should be sent to the sender.
    pub(crate) fn release(&mut self) -> Option<Window> {
        self.rx_window += Window(1);
        debug_assert!(self.rx_window > self.tx_window);

        if self.rx_window <= Window(0) {
            return None;
        }

        // We don't want to send window updates for tiny changes, but instead
        // aggregate them when the changes are significant.
        let threshold = Window(self.tx_window.0 / UNCLAIMED_DENOMINATOR * UNCLAIMED_NUMERATOR);
        let release = self.rx_window - self.tx_window;

        if release >= threshold {
            self.tx_window += release;
            debug_assert_eq!(self.rx_window, self.tx_window);
            Some(release)
        } else {
            None
        }
    }
}

// TODO: should `Add` and `Sub` be saturating?
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[derive(Add, AddAssign, Sub, SubAssign, Serialize, Deserialize)]
pub(crate) struct Window(i32);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tx_flow_control() {
        let mut fc = TxFlowControl::new(Window(2));
        fc.spend();
        assert_eq!(fc.release(Window(1)), 1);

        for _ in 0..5 {
            fc.spend();
        }
        assert_eq!(fc.release(Window(1)), 0);
        assert_eq!(fc.release(Window(4)), 2);
    }

    // Checks that the number of sent window updates is small.
    #[test]
    fn rx_flow_control() {
        // Real-time case: a system is stable, incoming rate = processing rate.
        let mut rc = RxFlowControl::new(Window(1000));
        let total = 1000;
        let sent = (0..total).fold(0, |sent, _| {
            rc.spend();
            sent + rc.release().is_some() as u32
        });
        let ratio = sent as f64 / total as f64;
        assert!(ratio < 0.01, "{}", ratio);

        // Starting once an actor is unstuck.
        let mut rc = RxFlowControl::new(Window(0));
        let total = 2000;
        let sent = (0..total).fold(0, |sent, _| sent + rc.release().is_some() as u32);
        let ratio = sent as f64 / total as f64;
        assert!(ratio < 0.01, "{}", ratio);
    }
}
