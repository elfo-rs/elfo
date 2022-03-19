#[derive(Clone)]
pub(crate) struct Budget(u8);

impl Default for Budget {
    fn default() -> Self {
        Self(u8::MAX)
    }
}

impl Budget {
    pub(crate) async fn acquire(&mut self) {
        if self.0 == 0 {
            tokio::task::yield_now().await;
            *self = Self::default();
        }

        self.0 -= 1;
    }
}
