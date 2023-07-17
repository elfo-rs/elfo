pub(crate) const COMPRESSED_DATA_BUFFER_CAPACITY: usize = 64 * 1024;
pub(crate) const DECOMPRESSED_DATA_BUFFER_CAPACITY: usize = 128 * 1024;

const BUFFER_CHUNK_SIZE: usize = 512;

pub(crate) struct ReadBuffer {
    buffer: Vec<u8>,
    position: usize,
    filled: usize,
}

impl ReadBuffer {
    pub(crate) fn with_capacity(capacity: usize) -> Self {
        Self {
            buffer: Vec::with_capacity(capacity),
            position: 0,
            filled: 0,
        }
    }

    pub(crate) fn as_slice(&self) -> &[u8] {
        &self.buffer[self.position..self.filled]
    }

    pub(crate) fn remaining(&mut self) -> &mut [u8] {
        &mut self.buffer[self.filled..]
    }

    pub(crate) fn mark_filled(&mut self, count: usize) {
        debug_assert!(self.filled + count <= self.buffer.len());
        self.filled += count;
    }

    pub(crate) fn consume(&mut self, count: usize) {
        debug_assert!(self.position + count <= self.filled);
        self.position += count;
    }

    pub(crate) fn len(&self) -> usize {
        self.filled - self.position
    }

    pub(crate) fn extend_to_contain(&mut self, new_length: usize) {
        // We round up the number of requested bytes to be a multiple of the chunk size
        // to avoid reading too few bytes per syscall.
        let current_length = self.len();
        debug_assert!(new_length > current_length);
        let rounded_new_length =
            ((new_length + BUFFER_CHUNK_SIZE - 1) / BUFFER_CHUNK_SIZE) * BUFFER_CHUNK_SIZE;
        let additional = rounded_new_length - current_length;

        let postfix_free_space = self.buffer.len() - self.filled;
        if additional <= postfix_free_space {
            // There is enough space in the end of the buffer already.
        } else {
            // Shift the data into the beginning of the buffer to be able to reuse space at
            // the prefix.
            unsafe {
                let buffer_start = self.buffer.as_mut_ptr();
                let data_start = buffer_start.add(self.position);
                std::ptr::copy(data_start, buffer_start, current_length);
            }

            self.position = 0;
            self.filled = current_length;

            if rounded_new_length > self.buffer.len() {
                // If there is still not enough space after the shift, we need to
                // allocate more space.
                self.buffer.resize(rounded_new_length, 0);
            }
        }
    }
}
