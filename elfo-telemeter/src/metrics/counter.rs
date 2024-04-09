use super::MetricKind;

pub(crate) struct Counter(u64);

impl MetricKind for Counter {
    type Output = u64;
    type Shared = ();
    type Value = u64;

    fn new(_: Self::Shared) -> Self {
        Self(0)
    }

    fn update(&mut self, value: Self::Value) {
        self.0 += value;
    }

    fn merge(self, out: &mut Self::Output) -> usize {
        *out += self.0;
        0
    }
}
