use tokio::time::Instant;

/// Helper type which suggests when new timer should be
/// created.
pub(crate) struct AdviseTimer {
    readvise_at: Instant,
}

impl AdviseTimer {
    pub(crate) fn new() -> Self {
        Self {
            readvise_at: Instant::now(),
        }
    }

    pub(crate) fn feed(&mut self, advise: Instant) -> Option<Instant> {
        if advise >= self.readvise_at {
            self.readvise_at = advise;
            Some(advise)
        } else {
            None
        }
    }
}
