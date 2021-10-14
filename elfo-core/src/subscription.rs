use parking_lot::RwLock;
use tracing::warn;

use crate::{addr::Addr, context::Context, message::Message};

pub(crate) struct SubscriptionManager {
    ctx: Context,
    subscribers: RwLock<Vec<Addr>>,
}

impl SubscriptionManager {
    pub(crate) fn new(ctx: Context) -> Self {
        Self {
            ctx,
            subscribers: RwLock::new(Vec::new()),
        }
    }

    pub(crate) fn add(&self, addr: Addr) {
        self.subscribers.write().push(addr);
    }

    pub(crate) fn remove(&self, addr: Addr) {
        self.subscribers.write().retain(|stored| *stored != addr);
    }

    pub(crate) fn send(&self, message: impl Message) {
        let subscribers = self.subscribers.read();

        let (last, other) = ward!(subscribers.split_last());
        let mut subs_to_remove = Vec::new();

        for addr in other {
            if self.ctx.try_send_to(*addr, message.clone()).is_err() {
                subs_to_remove.push(*addr);
            }
        }

        if self.ctx.try_send_to(*last, message).is_err() {
            subs_to_remove.push(*last);
        }

        drop(subscribers);

        if !subs_to_remove.is_empty() {
            self.subscribers.write().retain(|addr| {
                let to_remove = subs_to_remove.contains(addr);
                if to_remove {
                    warn!(%addr, "message cannot be sent, unsubscribing");
                }
                !to_remove
            });
        }
    }
}
