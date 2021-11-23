use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

use metrics::Key;
use pin_project::pin_project;
use quanta::Instant;

static KEY: Key = Key::from_static_name("elfo_busy_time_seconds");

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
            recorder.record_histogram(&KEY, elapsed.as_secs_f64());
            res
        } else {
            this.inner.poll(cx)
        }
    }
}
