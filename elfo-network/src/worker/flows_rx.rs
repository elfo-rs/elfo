use std::collections::VecDeque;

use fxhash::FxHashMap;
use metrics::{decrement_gauge, increment_gauge};
use tracing::{debug, info};

use elfo_core::{Addr, Envelope};

use super::flow_control::RxFlowControl;
use crate::protocol::internode;

// TODO: `CloseFlow` ratelimiting.
// TODO: add `kind="Routed|Direct"` to the `elfo_network_rx_flows`.
// TODO: add `stability="Stable|Unstable"` to the `elfo_network_rx_flows`.

// For direct messages:
// * acquire_direct(true)
// * release_direct() (when a message is sent)
//
// For routed messages:
// * acquire_routed(true)
// * for each recipient:
//   - acquire_direct(false)
//   - acquire_routed(false)
//   - release_direct() (when a message is sent)
//   - release_routed() (when a message is sent)
// * release_routed()
pub(super) struct RxFlows {
    // local addr => flow data
    map: FxHashMap<Addr, RxFlowData>,
    routed_control: RxFlowControl,
    routed_used: bool,
    initial_window: i32,
}

impl Drop for RxFlows {
    fn drop(&mut self) {
        if self.routed_used {
            decrement_gauge!("elfo_network_rx_flows", 1.);
        }
    }
}

struct RxFlowData {
    control: RxFlowControl,
    /// If actor's mailbox is full, the message is queued here.
    /// The second element of the tuple is `true` if message was routed.
    queue: Option<VecDeque<(Envelope, bool)>>,
}

impl Drop for RxFlowData {
    fn drop(&mut self) {
        decrement_gauge!("elfo_network_rx_flows", 1.);
    }
}

impl RxFlows {
    pub(super) fn new(initial_window: i32) -> Self {
        Self {
            map: Default::default(),
            routed_control: RxFlowControl::new(initial_window),
            routed_used: false,
            initial_window,
        }
    }

    pub(super) fn get_flow(&mut self, addr: Addr) -> Option<RxFlow<'_>> {
        debug_assert!(addr.is_local());

        self.map.get_mut(&addr).map(|flow| RxFlow { addr, flow })
    }

    pub(super) fn get_or_create_flow(&mut self, addr: Addr) -> RxFlow<'_> {
        debug_assert!(addr.is_local());

        let initial_window = self.initial_window;
        let flow = self.map.entry(addr).or_insert_with(|| {
            increment_gauge!("elfo_network_rx_flows", 1.);
            RxFlowData {
                control: RxFlowControl::new(initial_window),
                queue: None,
            }
        });

        RxFlow { addr, flow }
    }

    pub(super) fn acquire_routed(&mut self, tx_knows: bool) {
        self.routed_control.do_acquire(tx_knows);

        if !self.routed_used {
            increment_gauge!("elfo_network_rx_flows", 1.);
            self.routed_used = true;
        }
    }

    pub(super) fn release_routed(&mut self) -> Option<internode::UpdateFlow> {
        self.routed_control
            .release()
            .map(|delta| internode::UpdateFlow {
                addr: Addr::NULL,
                window_delta: delta,
            })
    }

    pub(super) fn dequeue(&mut self, addr: Addr) -> Option<(Envelope, bool)> {
        debug_assert!(addr.is_local());

        let flow = self.map.get_mut(&addr)?;
        let queue = flow.queue.as_mut()?;
        let pair = queue.pop_front();

        if pair.is_none() {
            info!(
                message = "destination actor is stable now, moving to real-time processing",
                addr = %addr,
            );
            flow.queue = None;
        }

        pair
    }

    pub(super) fn close(&mut self, addr: Addr) -> Option<internode::CloseFlow> {
        debug_assert!(addr.is_local());

        let flow = self.map.remove(&addr)?;
        debug!(
            message = "flow closed",
            addr = %addr,
            dropped = flow.queue.as_ref().map_or(0, |q| q.len()),
        );
        Some(internode::CloseFlow {
            addr: addr.into_remote(),
        })
    }
}

#[must_use]
pub(super) struct RxFlow<'a> {
    addr: Addr,
    flow: &'a mut RxFlowData,
}

impl RxFlow<'_> {
    pub(super) fn is_stable(&self) -> bool {
        self.flow.queue.is_none()
    }

    pub(super) fn acquire_direct(&mut self, tx_knows: bool) {
        self.flow.control.do_acquire(tx_knows);
    }

    pub(super) fn release_direct(&mut self) -> Option<internode::UpdateFlow> {
        self.flow
            .control
            .release()
            .map(|delta| internode::UpdateFlow {
                addr: self.addr.into_remote(),
                window_delta: delta,
            })
    }

    pub(super) fn enqueue(self, envelope: Envelope, routed: bool) {
        let addr = self.addr;
        self.flow
            .queue
            .get_or_insert_with(|| {
                info!(addr = %addr, "destination actor is full, queueing");
                VecDeque::new()
            })
            .push_back((envelope, routed));
    }
}
