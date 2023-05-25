use crate::{address_book::Addr, envelope::Envelope};

pub(crate) struct Remote {
    pub(crate) try_send: Box<dyn Fn(Option<Addr>, Envelope) -> RemoteRouteReport + Send + Sync>,
    pub(crate) send: Box<dyn Fn(Option<Addr>, Envelope) -> RemoteRouteReport + Send + Sync>,
}

pub enum RemoteRouteReport {
    Done,
    Wait(()),
    Closed(Envelope),
}
