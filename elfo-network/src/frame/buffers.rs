pub(crate) const COMPRESSED_DATA_BUFFER_CAPACITY: usize = 64 * 1024;
pub(crate) const DECOMPRESSED_DATA_BUFFER_CAPACITY: usize = 128 * 1024;

pub(crate) struct ReadBuffer {
    buffer: Vec<u8>,
    filled_start: usize,
    filled_end: usize,
}

impl ReadBuffer {
    pub(crate) fn with_capacity(capacity: usize) -> Self {
        Self {
            buffer: vec![0; capacity],
            filled_start: 0,
            filled_end: 0,
        }
    }

    pub(crate) fn filled_slice(&self) -> &[u8] {
        &self.buffer[self.filled_start..self.filled_end]
    }

    /// Mark `count` bytes at the end of the buffer as filled with data.
    pub(crate) fn extend_filled(&mut self, count: usize) {
        assert!(self.filled_end + count <= self.buffer.len());
        self.filled_end += count;
    }

    /// Mark `count` bytes at the start of the filled data as ready to be
    /// reused for free space.
    pub(crate) fn consume_filled(&mut self, count: usize) {
        assert!(self.filled_start + count <= self.filled_end);
        self.filled_start += count;

        // If there is no more filled data, move indexes to the start to avoid shifting
        // data later in `extend_to_contain`
        if self.filled_start == self.filled_end {
            self.filled_start = 0;
            self.filled_end = 0;
        }
    }

    pub(crate) fn filled_len(&self) -> usize {
        self.filled_end - self.filled_start
    }

    /// Ensures that the buffer size is enough to contain at least `capacity`
    /// bytes of filled data. Returns the slice for the caller to fill with
    /// data.
    ///
    /// See `extend_filled` and `consume_filled`.
    pub(crate) fn extend_to_contain(&mut self, capacity: usize) -> &mut [u8] {
        let filled_len = self.filled_len();

        // Shift the data into the beginning of the buffer to reclaim unused space
        // before `self.position`, if there is anything to shift.
        if filled_len > 0 {
            assert!(self.filled_start < self.buffer.len());
            assert!(self.filled_start + filled_len <= self.buffer.len());
            // SAFETY:
            // 1. `self.buffer` is initialized, so the `buffer_start` pointer is valid
            // 2. Due to the assertion above, `self.filled_start` is a valid index into
            // `self.buffer`. Thus, `data_start` is a valid pointer.
            // 3. Due to the assertion above, there are at least `filled_len` bytes in the
            // buffer after `self.filled_start`. Also, since `self.filled_start >= 0`, there
            // are at least `filled_len` bytes in the buffer total. Thus, `std::ptr::copy`
            // will only read bytes from `self.buffer`.
            // 4. Finally, we are explicitly allowed to copy overlapping memory regions
            // using `std::ptr::copy`.
            unsafe {
                let buffer_start = self.buffer.as_mut_ptr();
                let data_start = buffer_start.add(self.filled_start);
                std::ptr::copy(data_start, buffer_start, filled_len);
            }
        }

        self.filled_start = 0;
        self.filled_end = filled_len;

        if capacity > self.buffer.len() {
            self.buffer.resize(capacity, 0);
        }

        &mut self.buffer[self.filled_end..]
    }
}
