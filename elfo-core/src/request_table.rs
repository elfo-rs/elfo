use std::{fmt, marker::PhantomData};

use futures_intrusive::sync::ManualResetEvent;
use parking_lot::Mutex;
use slotmap::{new_key_type, Key, SlotMap};
use smallvec::SmallVec;

use crate::{addr::Addr, address_book::AddressBook, envelope::Envelope};

pub(crate) struct RequestTable {
    owner: Addr,
    notifier: ManualResetEvent,
    requests: Mutex<SlotMap<RequestId, RequestInfo>>,
}

assert_impl_all!(RequestTable: Sync);

type Data = SmallVec<[Option<Envelope>; 1]>;

#[derive(Default)]
struct RequestInfo {
    remainder: usize,
    data: Data,
    collect_all: bool,
}

new_key_type! {
    pub struct RequestId;
}

impl RequestTable {
    pub(crate) fn new(owner: Addr) -> Self {
        Self {
            owner,
            notifier: ManualResetEvent::new(false),
            requests: Mutex::new(SlotMap::default()),
        }
    }

    pub(crate) fn new_request(&self, book: AddressBook, collect_all: bool) -> ResponseToken<()> {
        let mut requests = self.requests.lock();
        let request_id = requests.insert(RequestInfo {
            remainder: 1,
            data: Data::new(),
            collect_all,
        });
        ResponseToken::new(self.owner, request_id, book)
    }

    pub(crate) fn clone_token(&self, token: &ResponseToken<()>) -> Option<ResponseToken<()>> {
        debug_assert_eq!(token.sender, self.owner);
        let mut requests = self.requests.lock();
        requests.get_mut(token.request_id)?.remainder += 1;
        let book = token.book.clone();
        Some(ResponseToken::new(token.sender, token.request_id, book))
    }

    pub(crate) fn respond(&self, mut token: ResponseToken<()>, envelope: Envelope) {
        self.resolve(token.sender, token.request_id, Some(envelope));
        token.forget();
    }

    pub(crate) async fn wait(&self, request_id: RequestId) -> Data {
        let mut n = 0;

        loop {
            self.notifier.wait().await;

            {
                let mut requests = self.requests.lock();
                let request = requests.get(request_id).expect("unknown request");

                if request.remainder == 0 {
                    let info = requests.remove(request_id).expect("under lock");

                    // TODO: use another approach.
                    if requests.values().all(|info| info.remainder != 0) {
                        self.notifier.reset();
                    }

                    break info.data;
                }
            }

            // XXX: dirty fix to avoid high CPU usage.
            n += 1;
            if n % 10 == 0 {
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            } else {
                tokio::task::yield_now().await;
            }
        }
    }

    fn resolve(&self, sender: Addr, request_id: RequestId, envelope: Option<Envelope>) {
        // TODO: should we have another strategy for panics?
        debug_assert_eq!(sender, self.owner);
        let mut requests = self.requests.lock();

        // `None` here means the request was with `collect_all = false` and
        // the response has been recieved already.
        let request = ward!(requests.get_mut(request_id));

        // Extra responses (in `any` case).
        if request.remainder == 0 {
            debug_assert!(!request.collect_all);
            return;
        }

        if !request.collect_all {
            if envelope.is_some() {
                request.data.push(envelope);
                request.remainder = 0;
                self.notifier.set();
                return;
            }
        } else {
            request.data.push(envelope);
        }

        request.remainder -= 1;
        if request.remainder == 0 {
            self.notifier.set();
        }
    }
}

#[must_use]
pub struct ResponseToken<T> {
    pub(crate) sender: Addr,
    pub(crate) request_id: RequestId,
    book: AddressBook,
    marker: PhantomData<T>,
}

impl ResponseToken<()> {
    pub(crate) fn new(sender: Addr, request_id: RequestId, book: AddressBook) -> Self {
        Self {
            sender,
            request_id,
            book,
            marker: PhantomData,
        }
    }

    pub(crate) fn into_typed<T>(mut self) -> ResponseToken<T> {
        let token = ResponseToken {
            sender: self.sender,
            request_id: self.request_id,
            book: self.book.clone(),
            marker: PhantomData,
        };
        self.forget();
        token
    }
}

impl<R> ResponseToken<R> {
    pub(crate) fn forgotten(book: AddressBook) -> Self {
        Self {
            sender: Addr::NULL,
            request_id: RequestId::null(),
            book,
            marker: PhantomData,
        }
    }

    pub(crate) fn into_untyped(mut self) -> ResponseToken<()> {
        let token = ResponseToken {
            sender: self.sender,
            request_id: self.request_id,
            book: self.book.clone(),
            marker: PhantomData,
        };
        self.forget();
        token
    }

    pub(crate) fn is_forgotten(&self) -> bool {
        self.request_id == RequestId::null()
    }

    fn forget(&mut self) {
        self.request_id = RequestId::null();
    }
}

impl<T> Drop for ResponseToken<T> {
    fn drop(&mut self) {
        // We use the special value of `RequestId` to reduce memory usage.
        if self.request_id.is_null() {
            return;
        }

        let object = ward!(self.book.get(self.sender));
        let actor = ward!(object.as_actor());
        actor
            .request_table()
            .resolve(self.sender, self.request_id, None);
    }
}

impl<T> fmt::Debug for ResponseToken<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ResponseToken").finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::Arc;

    use elfo_macros::message;

    use crate::{actor::ActorMeta, assert_msg_eq, envelope::MessageKind, scope::Scope};

    #[message(elfo = crate)]
    #[derive(PartialEq)]
    struct Num(u32);

    fn envelope(addr: Addr, num: Num) -> Envelope {
        Scope::test(
            addr,
            Arc::new(ActorMeta {
                group: "test".into(),
                key: String::new(),
            }),
        )
        .sync_within(|| Envelope::new(num, MessageKind::Regular { sender: addr }).upcast())
    }

    #[tokio::test]
    async fn one_request_one_response() {
        let addr = Addr::from_bits(1);
        let table = Arc::new(RequestTable::new(addr));
        let book = AddressBook::new();

        for _ in 0..3 {
            let token = table.new_request(book.clone(), true);
            let request_id = token.request_id;

            let table1 = table.clone();
            tokio::spawn(async move {
                table1.respond(token, envelope(addr, Num(42)));
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
        let request_id = token.request_id;

        let n = 5;
        for i in 1..n {
            let table1 = table.clone();
            let token = table.clone_token(&token).unwrap();
            tokio::spawn(async move {
                if !ignore {
                    table1.respond(token, envelope(addr, Num(i)));
                } else {
                    // TODO: test a real `Drop`.
                    table1.resolve(addr, request_id, None);
                }
            });
        }

        if !ignore {
            table.respond(token, envelope(addr, Num(0)));
        } else {
            // TODO: test a real `Drop`.
            table.resolve(addr, request_id, None);
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

        for (i, envelope) in data.drain(..).enumerate() {
            if ignore {
                assert!(envelope.is_none());
            } else {
                assert_msg_eq!(envelope.unwrap(), Num(i as u32));
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
        let token1 = table.clone_token(&token).unwrap();
        let request_id = token.request_id;

        let table1 = table.clone();
        tokio::spawn(async move {
            table1.respond(token, envelope(addr, Num(42)));
        });

        let _data = table.wait(request_id).await;
        table.respond(token1, envelope(addr, Num(43)));
    }
}
