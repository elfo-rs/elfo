use fxhash::FxHashMap;
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
            self.map.remove(&(owner, request_id))
        } else {
            self.map
                .get(&(owner, request_id))
                .map(ResponseToken::duplicate)
        }
    }
}
