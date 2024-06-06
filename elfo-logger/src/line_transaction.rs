use crate::line_buffer::{CurrentLine, DirectWrite};

use super::line_buffer::LineBuffer;

/// This is needed due to rust's closure lifetimes binding limitation. Consider,
/// for example, the following implementation:
/// ```no_compile
/// triat LineFactory<'a>: FnOnce(&'a LineBuffer) -> Self::Line {
///     type Line: Line + 'a;
/// }
/// ```
///
/// and the following signature:
/// ```no_compile
/// fn use_factory(buffer: &mut LineBuffer, f: impl for<'a> LineFactory<'a>);
/// ```
///
/// Today, on stable, it's not possible, since rust can't infer that return type
/// lifetime depends on `buffer`'s HRTB lifetime: `use_factory(&mut buffer,
/// |buf: &mut LineBuffer| buf.direct_write())` - this will not compile.
// This is silly fix
pub(crate) trait LineFactory {
    type Line<'a>: Line + 'a;
    const STOP: bool;

    fn create_line(buf: &mut LineBuffer) -> Self::Line<'_>;
}

pub(crate) struct TryDirectWrite;
pub(crate) struct UseSlowPath;

impl LineFactory for TryDirectWrite {
    type Line<'a> = DirectWrite<'a>;

    const STOP: bool = false;

    fn create_line(buf: &mut LineBuffer) -> Self::Line<'_> {
        buf.direct_write()
    }
}
impl LineFactory for UseSlowPath {
    type Line<'a> = CurrentLine<'a>;

    const STOP: bool = true;

    fn create_line(buf: &mut LineBuffer) -> Self::Line<'_> {
        buf.new_line()
    }
}

pub(crate) trait Line {
    fn meta_mut(&mut self) -> &mut String;
    fn payload_mut(&mut self) -> &mut String;
    fn fields_mut(&mut self) -> &mut String;

    fn total_wrote(&self) -> usize;
    fn discard(self);
}
