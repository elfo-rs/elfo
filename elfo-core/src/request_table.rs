use std::{fmt, marker::PhantomData, sync::Arc};

use idr_ebr::EbrGuard;
use parking_lot::Mutex;
use slotmap::{new_key_type, Key, SlotMap};
use smallvec::SmallVec;
use tokio::sync::Notify;

use crate::{
    address_book::AddressBook, envelope::Envelope, errors::RequestError, message::AnyMessage,
    tracing::TraceId, Addr,
};

// === RequestId ===

new_key_type! {
    pub struct RequestId;
}

impl RequestId {
    #[doc(hidden)]
    #[inline]
    pub fn to_ffi(self) -> u64 {
        self.data().as_ffi()
    }

    #[doc(hidden)]
    #[inline]
    pub fn from_ffi(id: u64) -> Self {
        slotmap::KeyData::from_ffi(id).into()
    }

    #[doc(hidden)]
    #[inline]
    pub fn is_null(&self) -> bool {
        Key::is_null(self)
    }
}

// === RequestTable ===

pub(crate) struct RequestTable {
    owner: Addr,
    notifier: Notify,
    requests: Mutex<SlotMap<RequestId, RequestData>>,
}

assert_impl_all!(RequestTable: Sync);

type Responses = SmallVec<[Result<Envelope, RequestError>; 1]>;

#[derive(Default)]
struct RequestData {
    remainder: usize,
    responses: Responses,
    collect_all: bool,
}

impl RequestData {
    /// Returns `true` if the request is done.
    fn push(&mut self, response: Result<Envelope, RequestError>) -> bool {
        // Extra responses (in `any` case).
        if self.remainder == 0 {
            // TODO: move to `ResponseToken` to avoid sending extra responses over network.
            debug_assert!(!self.collect_all);
            return false;
        }

        self.remainder -= 1;

        if self.collect_all {
            self.responses.push(response);
            return self.remainder == 0;
        }

        // `Any` request contains at most one related response.
        debug_assert!(self.responses.len() <= 1);

        let is_ok = response.is_ok();

        if self.responses.is_empty() {
            self.responses.push(response);
        }
        // Priority: `Ok(_)` > `Err(Ignored)` > `Err(Failed)`
        else if response.is_ok() {
            debug_assert!(self.responses[0].is_err());
            self.responses[0] = response;
        } else if let Err(RequestError::Ignored) = response {
            debug_assert!(self.responses[0].is_err());
            self.responses[0] = response;
        }

        // Received `Ok`, so prevent further responses.
        if is_ok {
            self.remainder = 0;
        }

        self.remainder == 0
    }
}

impl RequestTable {
    pub(crate) fn new(owner: Addr) -> Self {
        Self {
            owner,
            notifier: Notify::new(),
            requests: Mutex::new(SlotMap::default()),
        }
    }

    pub(crate) fn new_request(
        &self,
        book: AddressBook,
        trace_id: TraceId,
        collect_all: bool,
    ) -> ResponseToken {
        let mut requests = self.requests.lock();
        let request_id = requests.insert(RequestData {
            remainder: 1,
            responses: Responses::new(),
            collect_all,
        });
        ResponseToken::new(self.owner, request_id, trace_id, book)
    }

    pub(crate) fn cancel_request(&self, request_id: RequestId) {
        let mut requests = self.requests.lock();
        requests.remove(request_id);
    }

    pub(crate) async fn wait(&self, request_id: RequestId) -> Responses {
        loop {
            let waiting = self.notifier.notified();

            {
                let mut requests = self.requests.lock();
                let request = requests.get(request_id).expect("unknown request");

                if request.remainder == 0 {
                    break requests.remove(request_id).expect("under lock").responses;
                }
            }

            waiting.await;
        }
    }

    pub(crate) fn resolve(
        &self,
        mut token: ResponseToken,
        response: Result<Envelope, RequestError>,
    ) {
        // Do nothing for forgotten tokens.
        let data = ward!(token.data.take());
        let mut requests = self.requests.lock();

        // `None` here means the request was with `collect_all = false` and
        // the response has been recieved already.
        let request = ward!(requests.get_mut(data.request_id));

        if request.push(response) {
            // Actors can perform multiple requests in parallel using different
            // wakers, so we should wake all possible wakers up.
            self.notifier.notify_waiters();
        }
    }
}

// === ResponseToken ===

#[must_use]
pub struct ResponseToken<T = AnyMessage> {
    /// `None` if forgotten.
    data: Option<Arc<ResponseTokenData>>,
    received: bool,
    marker: PhantomData<T>,
}

struct ResponseTokenData {
    sender: Addr,
    request_id: RequestId,
    trace_id: TraceId,
    book: AddressBook,
}

impl ResponseToken {
    #[doc(hidden)]
    #[inline]
    pub fn new(sender: Addr, request_id: RequestId, trace_id: TraceId, book: AddressBook) -> Self {
        debug_assert!(!sender.is_null());
        debug_assert!(!request_id.is_null());

        Self {
            data: Some(Arc::new(ResponseTokenData {
                sender,
                request_id,
                trace_id,
                book,
            })),
            received: false,
            marker: PhantomData,
        }
    }

    /// # Panics
    /// If the token is forgotten.
    #[doc(hidden)]
    #[inline]
    pub fn trace_id(&self) -> TraceId {
        self.data.as_ref().map(|data| data.trace_id).unwrap()
    }

    /// # Panics
    /// If the token is forgotten.
    #[doc(hidden)]
    #[inline]
    pub fn sender(&self) -> Addr {
        self.data.as_ref().map(|data| data.sender).unwrap()
    }

    /// # Panics
    /// If the token is forgotten.
    #[doc(hidden)]
    #[inline]
    pub fn request_id(&self) -> RequestId {
        self.data.as_ref().map(|data| data.request_id).unwrap()
    }

    /// # Panics
    /// If the token is forgotten.
    #[doc(hidden)]
    #[inline]
    pub fn is_last(&self) -> bool {
        self.data.as_ref().map(Arc::strong_count).unwrap() <= 1
    }

    #[doc(hidden)]
    #[inline]
    pub fn into_received<T>(mut self) -> ResponseToken<T> {
        ResponseToken {
            data: self.data.take(),
            received: true,
            marker: PhantomData,
        }
    }

    #[doc(hidden)]
    #[inline]
    pub fn duplicate(&self) -> Self {
        Self {
            data: self.do_duplicate(),
            received: self.received,
            marker: PhantomData,
        }
    }

    #[doc(hidden)]
    #[inline]
    pub fn forget(mut self) {
        self.data = None;
    }

    fn do_duplicate(&self) -> Option<Arc<ResponseTokenData>> {
        let data = self.data.as_ref()?;

        if data.sender.is_local() {
            let guard = EbrGuard::new();
            let object = data.book.get(data.sender, &guard)?;
            let actor = object.as_actor()?;
            let mut requests = actor.request_table().requests.lock();
            requests.get_mut(data.request_id)?.remainder += 1;
        }

        Some(data.clone())
    }
}

impl<R> ResponseToken<R> {
    #[doc(hidden)]
    #[inline]
    pub fn forgotten() -> Self {
        Self {
            data: None,
            received: false,
            marker: PhantomData,
        }
    }

    pub(crate) fn into_untyped(mut self) -> ResponseToken {
        ResponseToken {
            data: self.data.take(),
            received: self.received,
            marker: PhantomData,
        }
    }

    #[doc(hidden)]
    #[inline]
    pub fn is_forgotten(&self) -> bool {
        self.data.is_none()
    }
}

impl<T> Drop for ResponseToken<T> {
    #[inline]
    fn drop(&mut self) {
        // Do nothing for forgotten tokens.
        let data = ward!(self.data.take());
        let book = data.book.clone();
        let guard = EbrGuard::new();
        let object = ward!(book.get(data.sender, &guard));
        let this = ResponseToken {
            data: Some(data),
            received: self.received,
            marker: PhantomData,
        };
        let err = if self.received {
            RequestError::Ignored
        } else {
            RequestError::Failed
        };

        object.respond(this, Err(err));
    }
}

impl<T> fmt::Debug for ResponseToken<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ResponseToken").finish()
    }
}

#[cfg(test)]
#[cfg(TODO)]
mod tests {
    use super::*;

    use std::sync::Arc;

    use crate::{actor::ActorMeta, assert_msg_eq, envelope::MessageKind, message, scope::Scope};

    #[message]
    #[derive(PartialEq)]
    struct Num(u32);

    fn envelope(addr: Addr, request_id: RequestId, num: Num) -> Envelope {
        Scope::test(
            addr,
            Arc::new(ActorMeta {
                group: "test".into(),
                key: String::new(),
            }),
        )
        .sync_within(|| {
            Envelope::new(
                num,
                MessageKind::Response {
                    sender: addr,
                    request_id,
                },
            )
            .upcast()
        })
    }

    #[tokio::test]
    async fn one_request_one_response() {
        let addr = Addr::from_bits(1);
        let table = Arc::new(RequestTable::new(addr));
        let book = AddressBook::new();

        for _ in 0..3 {
            let token = table.new_request(book.clone(), true);
            let request_id = token.request_id();

            let table1 = table.clone();
            tokio::spawn(async move {
                table1.resolve(token, Ok(envelope(addr, request_id, Num(42))));
            });

            let mut data = table.wait(request_id).await;

            assert_eq!(data.len(), 1);
            assert_msg_eq!(data.pop().unwrap().unwrap(), Num(42));
        }
    }

    async fn one_request_many_response(collect_all: bool, ignore: bool) {
        let addr = Addr::from_bits(1);
        let table = Arc::new(RequestTable::new(addr));
        let token = table.new_request(AddressBook::new(), collect_all);
        let request_id = token.request_id();

        let n = 5;
        for i in 1..n {
            let table1 = table.clone();
            let token = table.clone_token(&token).unwrap();
            assert_eq!(token.request_id, request_id);
            tokio::spawn(async move {
                if !ignore {
                    table1.resolve(request_id, Ok(envelope(addr, request_id, Num(i))));
                } else {
                    // TODO: test a real `Drop`.
                    table1.resolve(request_id, Err(RequestError::Ignored));
                }
            });
        }

        if !ignore {
            table.resolve(request_id, Ok(envelope(addr, request_id, Num(0))));
        } else {
            // TODO: test a real `Drop`.
            table.resolve(request_id, Err(RequestError::Ignored));
        }

        let mut data = table.wait(request_id).await;

        let expected_len = if ignore {
            0
        } else if collect_all {
            n as usize
        } else {
            1
        };
        assert_eq!(data.len(), expected_len);

        for (i, response) in data.drain(..).enumerate() {
            if ignore {
                assert!(response.is_err());
            } else {
                assert_msg_eq!(response.unwrap(), Num(i as u32));
            }
        }
    }

    #[tokio::test]
    async fn one_request_many_response_all() {
        one_request_many_response(true, false).await;
    }

    #[tokio::test]
    async fn one_request_many_response_all_ignored() {
        one_request_many_response(false, true).await;
    }

    #[tokio::test]
    async fn one_request_many_response_any() {
        one_request_many_response(false, false).await;
    }

    #[tokio::test]
    async fn one_request_many_response_any_ignored() {
        one_request_many_response(false, true).await;
    }

    // TODO: check many requests.
    // TODO: check `Drop`.

    #[tokio::test]
    async fn late_resolve() {
        let addr = Addr::from_bits(1);
        let table = Arc::new(RequestTable::new(addr));
        let book = AddressBook::new();

        let token = table.new_request(book.clone(), false);
        let _token1 = table.clone_token(&token).unwrap();
        let request_id = token.request_id;

        let table1 = table.clone();
        tokio::spawn(async move {
            table1.resolve(request_id, Ok(envelope(addr, request_id, Num(42))));
        });

        let _data = table.wait(request_id).await;
        table.resolve(request_id, Ok(envelope(addr, request_id, Num(43))));
    }
}
