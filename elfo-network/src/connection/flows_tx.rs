use dashmap::DashMap;
use fxhash::FxBuildHasher;
use metrics::{decrement_gauge, increment_gauge};
use tracing::{debug, warn};

use elfo_core::{
    remote::{SendNotified, SendNotify},
    Addr,
};
use elfo_utils::likely;

use super::flow_control::TxFlowControl;
use crate::protocol::internode;

#[derive(Default)]
pub(super) struct TxFlows {
    // remote or null addr => flow data
    map: DashMap<Addr, TxFlow, FxBuildHasher>,
    initial_window: i32,
}

struct TxFlow {
    control: TxFlowControl,
    waiters: SendNotify,
}

impl Drop for TxFlow {
    fn drop(&mut self) {
        decrement_gauge!("elfo_network_tx_flows", 1.);
    }
}

pub(super) enum TryAcquire {
    Done,
    Full,
    Closed,
}

pub(super) enum Acquire {
    Done,
    Full(SendNotified),
    Closed,
}

impl TxFlows {
    pub(super) fn new(initial_window: i32) -> Self {
        let this = Self {
            map: Default::default(),
            initial_window,
        };

        // A flow for the group is always present.
        this.add_flow_if_needed(Addr::NULL);
        this
    }

    pub(super) fn acquire(&self, addr: Addr) -> Acquire {
        debug_assert!(!addr.is_local());

        let Some(flow) = self.map.get(&addr) else {
            return Acquire::Closed;
        };

        if flow.control.try_acquire() {
            Acquire::Done
        } else {
            // `waiters.notify()` is called by `update_flow()` and `close_flow()`, which
            // take an exclusive lock to the flow. Thus, we cannot miss a
            // notification here.
            Acquire::Full(flow.waiters.notified())
        }
    }

    pub(super) fn try_acquire(&self, addr: Addr) -> TryAcquire {
        debug_assert!(!addr.is_local());

        let Some(flow) = self.map.get(&addr) else {
            return TryAcquire::Closed;
        };

        if flow.control.try_acquire() {
            TryAcquire::Done
        } else {
            TryAcquire::Full
        }
    }

    pub(super) fn do_acquire(&self, addr: Addr) -> bool {
        debug_assert!(!addr.is_local());

        let Some(flow) = self.map.get(&addr) else {
            return false;
        };

        flow.control.do_acquire();
        true
    }

    pub(super) fn add_flow_if_needed(&self, addr: Addr) {
        debug_assert!(!addr.is_local());

        if likely(self.map.contains_key(&addr)) {
            return;
        }

        self.map.entry(addr).or_insert_with(|| {
            let control = TxFlowControl::new(self.initial_window);
            increment_gauge!("elfo_network_tx_flows", 1.);
            TxFlow {
                control,
                waiters: SendNotify::default(),
            }
        });
        debug!(addr = %addr, "flow added");
    }

    pub(super) fn update_flow(&self, update: &internode::UpdateFlow) {
        // It's important to take an exclusive lock here to avoid race with `acquire()`.
        let Some(flow) = self.map.get_mut(&update.addr) else {
            warn!(addr = %update.addr, "received update for unknown flow");
            return;
        };

        if flow.control.release(update.window_delta) {
            flow.waiters.notify();
        }

        debug!(
            message = "flow updated",
            addr = %update.addr,
            window_delta = %update.window_delta,
        );
    }

    pub(super) fn close_flow(&self, close: &internode::CloseFlow) {
        let Some((addr, flow)) = self.map.remove(&close.addr) else {
            warn!(addr = %close.addr, "received close for unknown flow");
            return;
        };

        debug!(addr = %addr, "flow closed");
        flow.waiters.notify();
    }
}
