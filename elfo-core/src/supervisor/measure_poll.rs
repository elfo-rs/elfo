use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

use metrics::Key;
use pin_project::pin_project;
use quanta::Instant;

static BUSY_TIME_SECONDS: Key = Key::from_static_name("elfo_busy_time_seconds");
static ALLOCATED_BYTES: Key = Key::from_static_name("elfo_allocated_bytes_total");
static DEALLOCATED_BYTES: Key = Key::from_static_name("elfo_deallocated_bytes_total");

#[pin_project]
pub(crate) struct MeasurePoll<F> {
    #[pin]
    inner: F,
}

impl<F> MeasurePoll<F> {
    pub(crate) fn new(inner: F) -> Self {
        Self { inner }
    }
}

impl<F: Future> Future for MeasurePoll<F> {
    type Output = F::Output;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();

        if let Some(recorder) = metrics::try_recorder() {
            let start_time = Instant::now();
            let res = this.inner.poll(cx);
            let elapsed = Instant::now().duration_since(start_time);
            recorder.record_histogram(&BUSY_TIME_SECONDS, elapsed.as_secs_f64());
            crate::scope::with(|scope| {
                recorder.increment_counter(&ALLOCATED_BYTES, scope.take_allocated_bytes() as u64);
                recorder
                    .increment_counter(&DEALLOCATED_BYTES, scope.take_deallocated_bytes() as u64);
            });
            res
        } else {
            this.inner.poll(cx)
        }
    }
}
