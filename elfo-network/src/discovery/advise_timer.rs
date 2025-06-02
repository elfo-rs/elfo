use tokio::time::Instant;

/// Helper type which suggests when new timer should be
/// created.
pub(crate) struct AdviseTimer {
    readvise_at: Instant,
}

pub(crate) enum NewTimerSetup {
    Do { at: Instant },
    OldIsStillTicking,
}

impl AdviseTimer {
    pub(crate) fn new() -> Self {
        Self {
            readvise_at: Instant::now(),
        }
    }

    pub(crate) fn feed(&mut self, advise: Instant) -> NewTimerSetup {
        if advise >= self.readvise_at {
            self.readvise_at = advise;
            NewTimerSetup::Do { at: advise }
        } else {
            NewTimerSetup::OldIsStillTicking
        }
    }
}
