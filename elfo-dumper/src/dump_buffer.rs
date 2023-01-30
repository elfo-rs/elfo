use std::io;

use serde::Serialize;

const MIN_CHUNK_SIZE: usize = 128 * 1024;

// We should consider the limit and newlines, but the first one can be too
// large and the number of newlines cannot be calculated before serialization.
// So, just multiply the chunk's size by some coef, that's a good assumption.
const INITIAL_CAPACITY: usize = MIN_CHUNK_SIZE * 3 / 2;

pub(crate) struct DumpBuffer {
    buffer: Vec<u8>,
    max_dump_size: usize,
    need_to_clear: bool,
}

#[derive(Debug)]
pub(crate) enum AppendError {
    LimitExceeded(usize),
    SerializationFailed(serde_json::Error),
}

impl DumpBuffer {
    pub(crate) fn new(max_dump_size: usize) -> Self {
        Self {
            buffer: Vec::with_capacity(INITIAL_CAPACITY),
            max_dump_size,
            need_to_clear: false,
        }
    }

    pub(crate) fn configure(&mut self, max_dump_size: usize) {
        assert!(self.buffer.is_empty() || self.need_to_clear);

        if max_dump_size != self.max_dump_size {
            self.max_dump_size = max_dump_size;
            *self = Self::new(max_dump_size);
        }
    }

    pub(crate) fn append(&mut self, dump: &impl Serialize) -> Result<Option<&[u8]>, AppendError> {
        self.clear_if_needed();

        let len = self.buffer.len();
        let res = self.do_append(dump);

        if res.is_err() {
            self.buffer.truncate(len);
        }

        res?;
        self.buffer.push(b'\n');

        Ok(self.take_if_limit_exceeded(MIN_CHUNK_SIZE - 1))
    }

    pub(crate) fn take(&mut self) -> Option<&[u8]> {
        self.clear_if_needed();
        self.take_if_limit_exceeded(0)
    }

    fn clear_if_needed(&mut self) {
        if self.need_to_clear {
            self.buffer.clear();
            self.need_to_clear = false;
        }
    }

    fn do_append(&mut self, dump: &impl Serialize) -> Result<(), AppendError> {
        let limit = self.max_dump_size;
        let wr = LimitedWrite(&mut self.buffer, limit);

        match serde_json::to_writer(wr, dump) {
            Ok(()) => Ok(()),
            Err(err) if err.is_io() => Err(AppendError::LimitExceeded(limit)),
            Err(err) => Err(AppendError::SerializationFailed(err)),
        }
    }

    fn take_if_limit_exceeded(&mut self, limit: usize) -> Option<&[u8]> {
        if self.buffer.len() > limit {
            self.need_to_clear = true;
            Some(&self.buffer)
        } else {
            None
        }
    }
}

struct LimitedWrite<W>(W, usize);

impl<W: io::Write> io::Write for LimitedWrite<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if buf.len() > self.1 {
            return Ok(0);
        }

        self.1 -= buf.len();
        self.0.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_works() {
        let mut buffer = DumpBuffer::new(2 * INITIAL_CAPACITY);

        // Put some dumps without exceeding the chunk's size.
        assert!(buffer.take().is_none());
        assert!(buffer.append(&1).unwrap().is_none());
        assert!(buffer.append(&2).unwrap().is_none());
        assert!(buffer.append(&3).unwrap().is_none());
        assert_eq!(buffer.take().unwrap(), b"1\n2\n3\n");

        // Check that old data aren't used.
        assert!(buffer.take().is_none());
        assert!(buffer.append(&"1").unwrap().is_none());
        assert!(buffer.append(&"2").unwrap().is_none());
        assert!(buffer.append(&"3").unwrap().is_none());
        assert_eq!(buffer.take().unwrap(), b"\"1\"\n\"2\"\n\"3\"\n");
        assert!(buffer.take().is_none());

        // Put dumps with exceeding the chunk's size;
        let one = "1".repeat(MIN_CHUNK_SIZE / 3 * 2);
        let two = "2".repeat(MIN_CHUNK_SIZE / 3 * 2);
        let expected = format!("\"{one}\"\n\"{two}\"\n");
        assert!(buffer.append(&one).unwrap().is_none());
        let buf = buffer.append(&two).unwrap().unwrap();
        assert_eq!(buf, expected.as_bytes());

        // Check erroneous data aren't added to the buffer.
        #[derive(Serialize)]
        struct Sample {
            n: u32,
            s: String,
        }

        let small = Sample {
            n: 5,
            s: String::new(),
        };
        let big = Sample {
            n: 5,
            s: "1".repeat(50),
        };

        buffer.configure(50);
        assert!(buffer.append(&small).unwrap().is_none());
        assert!(matches!(
            buffer.append(&big).unwrap_err(),
            AppendError::LimitExceeded(_)
        ));
        assert!(buffer.append(&small).unwrap().is_none());
        assert!(matches!(
            buffer.append(&big).unwrap_err(),
            AppendError::LimitExceeded(_)
        ));
        assert_eq!(
            buffer.take().unwrap(),
            b"{\"n\":5,\"s\":\"\"}\n{\"n\":5,\"s\":\"\"}\n"
        );
    }
}
