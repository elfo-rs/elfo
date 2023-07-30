use fxhash::FxHashMap;
use metrics::{decrement_gauge, increment_gauge};
use tracing::error;

use elfo_core::{Addr, ResponseToken, _priv::RequestId};

#[derive(Default)]
pub(super) struct OutgoingRequests {
    map: FxHashMap<(Addr, RequestId), ResponseToken>,
}

impl OutgoingRequests {
    pub(super) fn add_token(&mut self, token: ResponseToken) {
        let (owner, request_id) = (token.sender(), token.request_id());

        debug_assert!(!token.is_forgotten());
        debug_assert!(owner.is_local());
        debug_assert!(!request_id.is_null());

        if self.map.insert((owner, request_id), token).is_some() {
            error!(
                message = "duplicate request found",
                owner = %owner,
                request_id = ?request_id,
            );
        } else {
            increment_gauge!("elfo_network_outgoing_requests", 1.);
        }
    }

    pub(super) fn get_token(
        &mut self,
        owner: Addr,
        request_id: RequestId,
        is_last_response: bool,
    ) -> Option<ResponseToken> {
        debug_assert!(owner.is_local());
        debug_assert!(!request_id.is_null());

        if is_last_response {
            let token = self.map.remove(&(owner, request_id));

            if token.is_some() {
                decrement_gauge!("elfo_network_outgoing_requests", 1.);
            }

            token
        } else {
            self.map
                .get(&(owner, request_id))
                .map(ResponseToken::duplicate)
        }
    }
}

impl Drop for OutgoingRequests {
    fn drop(&mut self) {
        // Filter zeros out here in order to avoid creating a metric
        // with a zero value if requests haven't been used at all.
        let count = self.map.len();
        if count > 0 {
            decrement_gauge!("elfo_network_outgoing_requests", count as f64);
        }
    }
}
