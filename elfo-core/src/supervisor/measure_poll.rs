use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

use idr_ebr::EbrGuard;
use metrics::Key;
use pin_project::pin_project;

use elfo_utils::time::Instant;

#[cfg(feature = "unstable-stuck-detection")]
use crate::stuck_detection::StuckDetector;

static BUSY_TIME_SECONDS: Key = Key::from_static_name("elfo_busy_time_seconds");
static ALLOCATED_BYTES: Key = Key::from_static_name("elfo_allocated_bytes_total");
static DEALLOCATED_BYTES: Key = Key::from_static_name("elfo_deallocated_bytes_total");

#[pin_project]
pub(crate) struct MeasurePoll<F> {
    #[pin]
    inner: F,
    #[cfg(feature = "unstable-stuck-detection")]
    stuck_detector: StuckDetector,
}

impl<F> MeasurePoll<F> {
    #[cfg(not(feature = "unstable-stuck-detection"))]
    pub(crate) fn new(inner: F) -> Self {
        Self { inner }
    }

    #[cfg(feature = "unstable-stuck-detection")]
    pub(crate) fn new(inner: F, stuck_detector: StuckDetector) -> Self {
        Self {
            inner,
            stuck_detector,
        }
    }
}

impl<F: Future> Future for MeasurePoll<F> {
    type Output = F::Output;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();

        // A controversial decision.
        // On the one hand, it makes internal guards slightly cheaper and more
        // predictable, i.e. don't start garbage collection inside an actor's code.
        // On the other hand, if some actor is stuck, garbage
        // collection stop working.
        // TODO: introduce a feature flag to disable this.
        let _ebr_guard = EbrGuard::new();

        crate::telemetry::enter();

        #[cfg(feature = "unstable-stuck-detection")]
        this.stuck_detector.enter();

        let result = if let Some(recorder) = metrics::try_recorder() {
            let start_time = Instant::now();
            crate::coop::reset(Some(start_time));
            let res = this.inner.poll(cx);
            recorder.record_histogram(&BUSY_TIME_SECONDS, start_time.elapsed_secs_f64());
            publish_alloc_metrics(recorder);
            res
        } else {
            crate::coop::reset(None);
            this.inner.poll(cx)
        };

        #[cfg(feature = "unstable-stuck-detection")]
        this.stuck_detector.exit();

        crate::telemetry::exit();

        #[allow(clippy::let_and_return)]
        result
    }
}

// TODO: Drop to ensure we call exit?

fn publish_alloc_metrics(recorder: &dyn metrics::Recorder) {
    crate::scope::with(|scope| {
        let allocated = scope.take_allocated_bytes();
        let deallocated = scope.take_deallocated_bytes();

        if allocated > 0 {
            recorder.increment_counter(&ALLOCATED_BYTES, allocated as u64);
        }
        if deallocated > 0 {
            recorder.increment_counter(&DEALLOCATED_BYTES, deallocated as u64);
        }
    });
}
