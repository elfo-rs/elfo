use crate::{
    address_book::Addr,
    envelope::Envelope,
    errors::{SendError, TrySendError},
};

#[stability::unstable]
pub trait RemoteHandle: Send + Sync + 'static {
    // TODO: fn send
    fn try_send(&self, recipient: Addr, envelope: Envelope) -> Result<(), TrySendError<Envelope>>;
    fn unbounded_send(
        &self,
        recipient: Addr,
        envelope: Envelope,
    ) -> Result<(), SendError<Envelope>>;
}
