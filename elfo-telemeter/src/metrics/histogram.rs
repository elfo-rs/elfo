use std::mem;

use super::MetricKind;
use crate::protocol::Distribution;

// TODO: *lazy* reservior sampling to improve memory usage?
pub(crate) struct Histogram(SegVec<f64>);

impl MetricKind for Histogram {
    type Output = Distribution;
    type Shared = ();
    type Value = f64;

    fn new(_: Self::Shared) -> Self {
        Self(SegVec::default())
    }

    fn update(&mut self, value: Self::Value) {
        self.0.push(value);
    }

    fn merge(self, out: &mut Self::Output) -> usize {
        let segments = self.0.into_segments();
        let mut additional_size = segments.capacity() * mem::size_of::<Vec<f64>>();

        for segment in segments {
            out.add(&segment);
            additional_size += segment.capacity() * mem::size_of::<f64>();
        }

        additional_size
    }
}

// A vector of segments to prevent reallocations.
// It improves tail latency, what's important for near RT actors.
struct SegVec<T> {
    active: Vec<T>,
    bag: Vec<Vec<T>>,
}

const INITIAL_LIMIT: usize = 8;
const GROWTH_FACTOR: usize = 4;

impl<T> Default for SegVec<T> {
    fn default() -> Self {
        Self {
            active: vec_with_exact_capacity(INITIAL_LIMIT),
            bag: Vec::new(),
        }
    }
}

impl<T> SegVec<T> {
    fn push(&mut self, value: T) {
        if self.active.len() == self.active.capacity() {
            self.new_segment();
        }

        #[cfg(test)]
        let capacity = self.active.capacity();
        self.active.push(value);
        #[cfg(test)]
        assert_eq!(self.active.capacity(), capacity);
    }

    #[cold]
    fn new_segment(&mut self) {
        let new = vec_with_exact_capacity(self.active.capacity() * GROWTH_FACTOR);
        let old = mem::replace(&mut self.active, new);
        self.bag.push(old);
    }

    fn into_segments(mut self) -> Vec<Vec<T>> {
        if !self.active.is_empty() {
            self.bag.push(self.active);
        }

        self.bag
    }
}

fn vec_with_exact_capacity<T>(capacity: usize) -> Vec<T> {
    let mut vec = Vec::new();
    vec.reserve_exact(capacity);
    debug_assert_eq!(vec.capacity(), capacity);
    vec
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seg_vec() {
        let mut vec = SegVec::default();
        let iters = 2024;

        for i in 0..iters {
            vec.push(i);
        }

        let segments = vec.into_segments();

        for (no, segment) in segments.iter().enumerate() {
            let expected_capacity = INITIAL_LIMIT * GROWTH_FACTOR.pow(no as u32);
            assert_eq!(segment.capacity(), expected_capacity);

            if no + 1 == segments.len() {
                assert_ne!(segment.len(), 0);
            } else {
                assert_eq!(segment.len(), segment.capacity());
            }
        }

        assert_eq!(
            segments.iter().map(|segment| segment.len()).sum::<usize>(),
            iters
        );

        segments
            .into_iter()
            .flatten()
            .enumerate()
            .for_each(|(i, v)| {
                assert_eq!(i, v);
            });
    }
}
