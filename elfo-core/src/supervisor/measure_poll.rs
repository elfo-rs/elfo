use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

use idr_ebr::EbrGuard;
use metrics::{GaugeValue, Key};
use pin_project::pin_project;

use elfo_utils::time::Instant;

#[cfg(feature = "unstable-stuck-detection")]
use crate::stuck_detection::StuckDetector;

static BUSY_TIME_SECONDS: Key = Key::from_static_name("elfo_busy_time_seconds");
static ALLOCATED_BYTES: Key = Key::from_static_name("elfo_allocated_bytes_total");
static DEALLOCATED_BYTES: Key = Key::from_static_name("elfo_deallocated_bytes_total");
static LINKED_BYTES: Key = Key::from_static_name("elfo_linked_bytes_total");

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

        #[cfg(feature = "unstable-stuck-detection")]
        this.stuck_detector.enter();

        let result = if let Some(recorder) = metrics::try_recorder() {
            let start_time = Instant::now();
            crate::coop::reset(Some(start_time));
            let res = this.inner.poll(cx);
            let elapsed = Instant::now().secs_f64_since(start_time);
            recorder.record_histogram(&BUSY_TIME_SECONDS, elapsed);
            publish_alloc_metrics(recorder);
            res
        } else {
            crate::coop::reset(None);
            this.inner.poll(cx)
        };

        #[cfg(feature = "unstable-stuck-detection")]
        this.stuck_detector.exit();

        #[allow(clippy::let_and_return)]
        result
    }
}

fn publish_alloc_metrics(recorder: &dyn metrics::Recorder) {
    crate::scope::with(|scope| {
        let allocated = scope.take_allocated_bytes();
        let deallocated = scope.take_deallocated_bytes();
        let linked = scope.take_linked_bytes_delta();

        if allocated > 0 {
            recorder.increment_counter(&ALLOCATED_BYTES, allocated as u64);
        }
        if deallocated > 0 {
            recorder.increment_counter(&DEALLOCATED_BYTES, deallocated as u64);
        }
        if linked != 0 {
            recorder.update_gauge(&LINKED_BYTES, GaugeValue::Increment(linked as f64));
        }
    });
}
