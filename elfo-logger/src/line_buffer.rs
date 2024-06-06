use std::mem;

use crate::line_transaction::Line;

// Repr

#[derive(Debug)]
struct Repr<'a> {
    buf: &'a mut LineBuffer,

    // We don't wanna write unfinished data, so there must be a way to
    // revert changes made by `CurrentLine`
    pre_start_buffer_size: usize,
}

// CurrentLine

#[derive(Debug)]
pub(crate) struct CurrentLine<'a>(Repr<'a>);

impl<'a> Line for CurrentLine<'a> {
    fn discard(self) {
        self.0.buf.buffer.truncate(self.0.pre_start_buffer_size);

        // It's okay to leak `CurrentLine` since it does not owns any resources, thus
        // freeing us from keeping additional run-time information about line status
        mem::forget(self);
    }

    fn total_wrote(&self) -> usize {
        self.len()
    }

    fn meta_mut(&mut self) -> &mut String {
        &mut self.0.buf.buffer
    }

    fn payload_mut(&mut self) -> &mut String {
        &mut self.0.buf.payload
    }

    fn fields_mut(&mut self) -> &mut String {
        &mut self.0.buf.fields
    }
}

impl<'a> CurrentLine<'a> {
    fn meta_len(&self) -> usize {
        self.0.buf.buffer.len() - self.0.pre_start_buffer_size
    }

    fn len(&self) -> usize {
        self.meta_len() + self.0.buf.payload.len() + self.0.buf.fields.len()
    }
}

impl<'a> CurrentLine<'a> {
    fn probe_size_limit(&mut self) {
        let len = self.len();
        let mut need_to_erase = if len > self.0.buf.max_line_size {
            len - self.0.buf.max_line_size
        } else {
            return;
        };

        let payload_len = self.0.buf.payload.len();
        let fields_len = self.0.buf.fields.len();
        let meta_len = self.meta_len();

        let payload_part = payload_len.min(need_to_erase);
        need_to_erase -= payload_part;
        let fields_part = fields_len.min(need_to_erase);
        need_to_erase -= fields_part;
        let meta_part = meta_len.min(need_to_erase);

        self.0.buf.payload.truncate(payload_len - payload_part);
        self.0.buf.fields.truncate(fields_len - fields_part);
        self.0
            .buf
            .buffer
            .truncate(self.0.buf.buffer.len() - meta_part);
    }
}

impl<'a> Drop for CurrentLine<'a> {
    fn drop(&mut self) {
        self.probe_size_limit();

        {
            let buffer = &mut self.0.buf.buffer;
            buffer.push_str(&self.0.buf.payload);
            buffer.push_str(&self.0.buf.fields);
            buffer.push('\n');
        }
    }
}

// DirectWrite

#[derive(Debug)]
pub(crate) struct DirectWrite<'a>(Repr<'a>);

impl<'a> Drop for DirectWrite<'a> {
    fn drop(&mut self) {
        self.0.buf.buffer.push('\n');
    }
}

impl Line for DirectWrite<'_> {
    fn discard(self) {
        self.0.buf.buffer.truncate(self.0.pre_start_buffer_size);

        mem::forget(self);
    }

    fn total_wrote(&self) -> usize {
        self.0.buf.buffer.len() - self.0.pre_start_buffer_size
    }

    fn meta_mut(&mut self) -> &mut String {
        &mut self.0.buf.buffer
    }

    fn payload_mut(&mut self) -> &mut String {
        &mut self.0.buf.buffer
    }

    fn fields_mut(&mut self) -> &mut String {
        &mut self.0.buf.buffer
    }
}

// LineBuffer

#[derive(Debug)]
pub(crate) struct LineBuffer {
    // Buffer will contain lines and partially written meta-info, such as `<timestamp> <level>
    // [<trace_id>]` `payload` and `fields` will be merged as soon as [`CurrentLine`] will be
    // finished
    pub(crate) buffer: String,

    payload: String,
    fields: String,

    pub(crate) max_line_size: usize,
}

impl Default for LineBuffer {
    fn default() -> Self {
        Self::new(usize::MAX)
    }
}

impl LineBuffer {
    /// Discards current buffers
    pub(crate) fn clear(&mut self) {
        self.buffer.clear();
        self.payload.clear();
        self.fields.clear();
    }

    fn create_repr(&mut self) -> Repr<'_> {
        let size = self.buffer.len();
        Repr {
            buf: self,
            pre_start_buffer_size: size,
        }
    }

    pub(crate) fn direct_write(&mut self) -> DirectWrite<'_> {
        DirectWrite(self.create_repr())
    }

    pub(crate) fn new_line(&mut self) -> CurrentLine<'_> {
        CurrentLine(self.create_repr())
    }

    pub(crate) fn with_buffer_capacity(capacity: usize, max_line_size: usize) -> Self {
        Self {
            buffer: String::with_capacity(capacity),
            payload: String::new(),
            fields: String::new(),
            max_line_size,
        }
    }

    pub(crate) const fn new(max_line_size: usize) -> Self {
        Self {
            buffer: String::new(),
            payload: String::new(),
            fields: String::new(),

            max_line_size,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{CurrentLine, LineBuffer};
    use crate::line_transaction::Line as _;

    fn put_msg(mut line: CurrentLine<'_>, meta: &str, payload: &str, fields: &str) {
        line.meta_mut().push_str(meta);
        line.payload_mut().push_str(payload);
        line.fields_mut().push_str(fields);
    }

    fn truncation_parametrized(limit: usize, cases: &[(&str, &str, &str, &str)]) {
        let mut buffer = LineBuffer::new(limit);
        for (meta, payload, fields, expected) in cases {
            let line = buffer.new_line();
            let before = line.0.pre_start_buffer_size;
            put_msg(line, meta, payload, fields);

            let after = buffer.buffer.len();
            let line = &buffer.buffer[before..after];
            assert_eq!(line, *expected);
        }
    }

    #[test]
    fn test_truncation() {
        truncation_parametrized(
            5,
            &[
                ("caffee ", "latte ", "ji ", "caffe\n"),
                // payload space dropped
                ("l ", "a ", "ke", "l ake\n"),
            ],
        );
        truncation_parametrized(
            10,
            &[
                // payload dropped
                ("hello ", "world ", "module=1234", "hello modu\n"),
            ],
        );
    }

    // When using 0 as line size limit, buffer must contain only newlines
    #[test]
    fn test_always_empty_string() {
        let mut buffer = LineBuffer::new(0);
        put_msg(buffer.new_line(), "Hello", "World", "!!!!");
        put_msg(
            buffer.new_line(),
            "12345",
            "I can count to 5!",
            "mood = good",
        );
        put_msg(buffer.new_line(), ":D", "Smiling face", "");

        assert_eq!(buffer.buffer, "\n\n\n");
    }
}
