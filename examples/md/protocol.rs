use elfo::prelude::*;

pub type Market = String;
// Obviously, it's more performant to use SSO-optimized strings or IDs.

#[message(ret = SubscribedToMd)]
pub struct SubscribeToMd {
    pub market: Market,
}

#[message]
pub struct SubscribedToMd {
    pub market: Market,
    pub snapshot: Vec<MdLevel>,
}

#[message(part)]
pub struct MdLevel(pub f64, pub f64);

#[message]
pub struct MdUpdated {
    pub market: Market,
    pub levels: Vec<MdLevel>,
}
