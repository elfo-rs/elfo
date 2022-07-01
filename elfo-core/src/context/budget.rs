#[derive(Clone)]
pub(crate) struct Budget(u8);

impl Default for Budget {
    fn default() -> Self {
        Self(64)
    }
}

impl Budget {
    pub(crate) async fn acquire(&mut self) {
        if self.0 == 0 {
            // We should reset the budget before `yield_now()` because
            // `select! { _ => ctx.recv() .. }` above can lock the branch forever.
            *self = Self::default();
            tokio::task::yield_now().await;
        }
    }

    pub(crate) fn decrement(&mut self) {
        // We use a saturating operation here because `try_recv()`
        // can be called many times without calling `Budget::acquire()`.
        self.0 = self.0.saturating_sub(1);
    }
}
