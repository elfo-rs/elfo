use std::sync::Arc;

use dashmap::DashMap;
use fxhash::FxBuildHasher;
use metrics::{decrement_gauge, increment_gauge};
use tracing::{debug, warn};

use elfo_core::remote::{SendNotified, SendNotify};
use elfo_utils::likely;

use super::flow_control::TxFlowControl;
use crate::{codec::format::NetworkAddr, protocol::internode};

pub(super) struct TxFlows {
    shared: Arc<Shared>,
    initial_window: i32,
}

#[derive(Default)]
struct Shared {
    // remote or null addr => flow data
    map: DashMap<NetworkAddr, TxFlow, FxBuildHasher>,
}

struct TxFlow {
    control: TxFlowControl,
    waiters: SendNotify,
}

impl TxFlows {
    pub(super) fn new(initial_window: i32) -> (Self, TxFlowsAcquirer) {
        let shared = Arc::new(Shared::default());
        let this = Self {
            shared: shared.clone(),
            initial_window,
        };

        // A flow for a group is always present.
        this.add_flow_if_needed(NetworkAddr::NULL);
        (this, TxFlowsAcquirer { shared })
    }

    pub(super) fn add_flow_if_needed(&self, addr: NetworkAddr) {
        if likely(self.shared.map.contains_key(&addr)) {
            return;
        }

        self.shared.map.entry(addr).or_insert_with(|| {
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
        let Some(flow) = self.shared.map.get_mut(&update.addr) else {
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
        let Some((addr, flow)) = self.shared.map.remove(&close.addr) else {
            warn!(addr = %close.addr, "received close for unknown flow");
            return;
        };

        debug!(addr = %addr, "flow closed");
        flow.waiters.notify();
    }
}

impl Drop for TxFlows {
    fn drop(&mut self) {
        let flow_count = self.shared.map.len();
        decrement_gauge!("elfo_network_tx_flows", flow_count as f64);
    }
}

pub(super) struct TxFlowsAcquirer {
    shared: Arc<Shared>,
}

impl TxFlowsAcquirer {
    pub(super) fn acquire(&self, addr: NetworkAddr) -> Acquire {
        let Some(flow) = self.shared.map.get(&addr) else {
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

    pub(super) fn try_acquire(&self, addr: NetworkAddr) -> TryAcquire {
        let Some(flow) = self.shared.map.get(&addr) else {
            return TryAcquire::Closed;
        };

        if flow.control.try_acquire() {
            TryAcquire::Done
        } else {
            TryAcquire::Full
        }
    }

    pub(super) fn do_acquire(&self, addr: NetworkAddr) -> bool {
        let Some(flow) = self.shared.map.get(&addr) else {
            return false;
        };

        flow.control.do_acquire();
        true
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
