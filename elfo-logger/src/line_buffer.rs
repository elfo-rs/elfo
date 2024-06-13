use std::mem;

use crate::line_transaction::Line;

pub(super) const TRUNCATED_MARKER: &str = " TRUNCATED";

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
    fn try_commit(self) -> bool {
        true
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
    fn probe_size_limit(&mut self) -> bool {
        let len = self.len();
        let mut need_to_erase = if len > self.0.buf.max_line_size {
            len - self.0.buf.max_line_size + TRUNCATED_MARKER.len()
        } else {
            return len + TRUNCATED_MARKER.len() <= self.0.buf.max_line_size;
        };

        let payload_len = self.0.buf.payload.len();
        let fields_len = self.0.buf.fields.len();
        let meta_len = self.meta_len();

        let payload_part = payload_len.min(need_to_erase);
        need_to_erase -= payload_part;
        need_to_erase = need_to_erase.saturating_sub(safe_truncate(
            &mut self.0.buf.payload,
            payload_len - payload_part,
        ));

        let fields_part = fields_len.min(need_to_erase);
        need_to_erase -= fields_part;
        need_to_erase = need_to_erase.saturating_sub(safe_truncate(
            &mut self.0.buf.fields,
            fields_len - fields_part,
        ));

        let meta_part = meta_len.min(need_to_erase);
        let truncate_meta_to = self.0.buf.buffer.len() - meta_part;

        safe_truncate(&mut self.0.buf.buffer, truncate_meta_to);
        self.len() + TRUNCATED_MARKER.len() <= self.0.buf.max_line_size
    }
}

impl<'a> Drop for CurrentLine<'a> {
    fn drop(&mut self) {
        let add_truncated_marker = self.probe_size_limit();

        {
            let buffer = &mut self.0.buf.buffer;
            buffer.push_str(&self.0.buf.payload);
            buffer.push_str(&self.0.buf.fields);
            if add_truncated_marker {
                buffer.push_str(TRUNCATED_MARKER);
            }
            buffer.push('\n');
        }
    }
}

// DirectWrite

#[derive(Debug)]
pub(crate) struct DirectWrite<'a>(Repr<'a>);

impl<'a> DirectWrite<'a> {
    fn total_bytes(&self) -> usize {
        self.0.buf.buffer.len() - self.0.pre_start_buffer_size
    }
}

impl<'a> Drop for DirectWrite<'a> {
    fn drop(&mut self) {
        self.0.buf.buffer.push('\n');
    }
}

impl Line for DirectWrite<'_> {
    fn try_commit(self) -> bool {
        if self.total_bytes() > self.0.buf.max_line_size {
            self.0.buf.buffer.truncate(self.0.pre_start_buffer_size);
            // It's okay to leak the `DirectWrite`, since it does not own any resources
            mem::forget(self);

            false
        } else {
            true
        }
    }

    // Nope, clippy, this is how it is supposed to work

    #[allow(clippy::misnamed_getters)]
    fn meta_mut(&mut self) -> &mut String {
        &mut self.0.buf.buffer
    }

    #[allow(clippy::misnamed_getters)]
    fn payload_mut(&mut self) -> &mut String {
        &mut self.0.buf.buffer
    }

    #[allow(clippy::misnamed_getters)]
    fn fields_mut(&mut self) -> &mut String {
        &mut self.0.buf.buffer
    }
}

// LineBuffer

#[derive(Debug, Default)]
pub(crate) struct LineBuffer {
    // Buffer will contain lines and partially written meta-info, such as `<timestamp> <level>
    // [<trace_id>]` `payload` and `fields` will be merged as soon as [`CurrentLine`] will be
    // finished
    buffer: String,
    payload: String,
    fields: String,

    max_line_size: usize,
}

impl LineBuffer {
    pub(crate) fn configure(&mut self, max_line_size: usize) {
        self.max_line_size = max_line_size;
    }

    /// Discards current buffers
    pub(crate) fn clear(&mut self) {
        self.buffer.clear();
        self.payload.clear();
        self.fields.clear();
    }

    pub(crate) fn as_str(&self) -> &str {
        self.buffer.as_str()
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

    pub(crate) fn with_capacity(capacity: usize, max_line_size: usize) -> Self {
        Self {
            buffer: String::with_capacity(capacity),
            payload: String::new(),
            fields: String::new(),
            max_line_size,
        }
    }
}

fn safe_truncate(text: &mut String, to: usize) -> usize {
    let mut boundary = to;
    while !text.is_char_boundary(boundary) {
        boundary -= 1;
    }

    text.truncate(boundary);
    to - boundary
}

#[cfg(test)]
mod tests {
    use super::{safe_truncate, CurrentLine, LineBuffer, TRUNCATED_MARKER};
    use crate::line_transaction::Line as _;

    fn put_msg(mut line: CurrentLine<'_>, meta: &str, payload: &str, fields: &str) {
        line.meta_mut().push_str(meta);
        line.payload_mut().push_str(payload);
        line.fields_mut().push_str(fields);
    }

    fn truncation_parametrized(limit: usize, cases: &[(&str, &str, &str, &str)]) {
        let mut buffer = LineBuffer::with_capacity(100, limit);
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
    fn test_safe_truncate_util() {
        for (original, truncate_to, expected) in [
            ("Hello 🫱🏻‍🫲🏿", 7, "Hello "),
            ("Hello 🫱 World 🫱", 7, "Hello "),
            ("Русский язык", 5, "Ру"),
            ("Русский язык", 23, "Русский язык"),
        ] {
            let mut original = original.to_owned();
            safe_truncate(&mut original, truncate_to);
            assert_eq!(original, expected);
        }
    }

    #[test]
    fn test_log_truncation() {
        truncation_parametrized(
            5 + TRUNCATED_MARKER.len(),
            &[
                ("caffee ", "latte ", "ji ", "caffe TRUNCATED\n"),
                // payload space dropped
                ("l ", "a ", "kekekekekekekekekeke", "l kek TRUNCATED\n"),
            ],
        );
        truncation_parametrized(
            10 + TRUNCATED_MARKER.len(),
            &[
                // payload dropped
                ("hello ", "world ", "module=1234", "hello modu TRUNCATED\n"),
            ],
        );
    }

    // When using 0 as line size limit, buffer must contain only newlines
    #[test]
    fn test_always_empty_string() {
        let mut buffer = LineBuffer::with_capacity(100, 0);
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
