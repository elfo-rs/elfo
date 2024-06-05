use std::{mem, num::NonZeroUsize};

// MaxLineSize

/// Max size of the log line, cannot be zero.
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
#[repr(transparent)]
pub(crate) struct MaxLineSize(pub(crate) NonZeroUsize);

// CurrentLine

#[derive(Debug)]
pub(crate) struct CurrentLine<'a> {
    buffer: &'a mut LineBuffer,

    // We don't wanna write unfinished data, so there must be a way to
    // revert changes made by `CurrentLine`
    pre_start_buffer_size: usize,
}

impl<'a> CurrentLine<'a> {
    fn meta_len(&self) -> usize {
        self.buffer.buffer.len() - self.pre_start_buffer_size
    }

    fn len(&self) -> usize {
        self.meta_len() + self.buffer.payload.len() + self.buffer.fields.len()
    }

    pub(crate) fn payload_mut(&mut self) -> &mut String {
        &mut self.buffer.buffer
    }

    pub(crate) fn buffer_mut(&mut self) -> &mut String {
        &mut self.buffer.buffer
    }

    pub(crate) fn fields_mut(&mut self) -> &mut String {
        &mut self.buffer.fields
    }
}

impl<'a> CurrentLine<'a> {
    fn probe_size_limit(&mut self) {
        let len = self.len();
        let mut need_to_erase =
            if let Some(size_limit) = self.buffer.max_line_size.map(|l| l.0.get()) {
                if len > size_limit {
                    len - size_limit
                } else {
                    return;
                }
            } else {
                return;
            };

        let payload_len = self.buffer.payload.len();
        let fields_len = self.buffer.fields.len();
        let meta_len = self.meta_len();

        let payload_part = payload_len.min(need_to_erase);
        need_to_erase -= payload_part;
        let fields_part = fields_len.min(need_to_erase);
        need_to_erase -= fields_part;
        let meta_part = meta_len.min(need_to_erase);

        self.buffer.payload.truncate(payload_len - payload_part);
        self.buffer.fields.truncate(fields_len - fields_part);
        self.buffer
            .buffer
            .truncate(self.buffer.buffer.len() - meta_part);
    }

    pub(crate) fn finish(mut self) {
        self.probe_size_limit();

        self.buffer
            .buffer
            .reserve(self.buffer.payload.len() + self.buffer.fields.len());
        self.buffer.buffer.extend(
            self.buffer
                .payload
                .chars()
                .chain(self.buffer.fields.chars())
                .chain(std::iter::once('\n')),
        );
        // It's okay to leak `CurrentLine` since it does not owns any resources, thus
        // freeing us from keeping additional run-time information about line status
        mem::forget(self);
    }
}

impl<'a> Drop for CurrentLine<'a> {
    fn drop(&mut self) {
        // It is guaranteed by contract that this drop will be called only when
        // line is unfinished, since [`CurrentLine::finish`] leaks [`CurrentLine`]
        self.buffer.buffer.truncate(self.pre_start_buffer_size);
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

    pub(crate) max_line_size: Option<MaxLineSize>,
}

impl Default for LineBuffer {
    fn default() -> Self {
        Self::new(None)
    }
}

impl LineBuffer {
    /// Discards current buffers
    pub(crate) fn clear(&mut self) {
        self.buffer.clear();
        self.payload.clear();
        self.fields.clear();
    }

    pub(crate) fn new_line(&mut self) -> CurrentLine<'_> {
        let size = self.buffer.len();
        CurrentLine {
            buffer: self,
            pre_start_buffer_size: size,
        }
    }

    pub(crate) const fn new(max_line_size: Option<MaxLineSize>) -> Self {
        Self {
            buffer: String::new(),
            payload: String::new(),
            fields: String::new(),

            max_line_size,
        }
    }
}
