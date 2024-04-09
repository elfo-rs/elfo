mod counter;
mod gauge;
mod histogram;

pub(crate) use self::{
    counter::Counter,
    gauge::{Gauge, GaugeOrigin},
    histogram::Histogram,
};

pub(crate) trait MetricKind: Sized {
    type Output;
    type Shared;
    type Value;

    fn new(shared: Self::Shared) -> Self;
    fn update(&mut self, value: Self::Value);

    /// Returns additional size to add to metrics.
    fn merge(self, out: &mut Self::Output) -> usize;
}
