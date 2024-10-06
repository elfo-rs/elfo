use std::{
    sync::{
        atomic::{AtomicU32, Ordering},
        Arc,
    },
    time::Duration,
};

use tokio::time::Instant;

// === IdleTracker ===

/// Measures the time since the last activity on a socket.
/// It's used to implement health checks based on idle timeout.
pub(crate) struct IdleTracker {
    track: IdleTrack,
    prev_value: u32,
    prev_time: Instant,
}

impl IdleTracker {
    pub(super) fn new() -> (Self, IdleTrack) {
        let track = IdleTrack(<_>::default());
        let this = Self {
            track: track.clone(),
            prev_value: track.get(),
            prev_time: Instant::now(),
        };

        (this, track)
    }

    /// Returns the elapsed time since the last `check()` call
    /// that observed any [`IdleTrack::update()`] calls.
    pub(crate) fn check(&mut self) -> Duration {
        let now = Instant::now();
        let new_value = self.track.get();

        if self.prev_value != new_value {
            self.prev_value = new_value;
            self.prev_time = now;
        }

        now.duration_since(self.prev_time)
    }
}

// === IdleTrack ===

#[derive(Clone)]
pub(super) struct IdleTrack(Arc<AtomicU32>);

impl IdleTrack {
    /// Marks this socket as non-idle.
    pub(super) fn update(&self) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }

    fn get(&self) -> u32 {
        self.0.load(Ordering::Relaxed)
    }
}
