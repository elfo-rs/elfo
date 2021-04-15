use std::fmt;

use derive_more::Deref;

#[derive(Debug, Clone, Copy, Default, Hash, PartialEq, Eq, Deref)]
// Spatial prefetcher is now pulling two lines at a time, so we use `align(128)`.
#[cfg_attr(any(target_arch = "x86_64", target_arch = "aarch64"), repr(align(128)))]
#[cfg_attr(
    not(any(target_arch = "x86_64", target_arch = "aarch64")),
    repr(align(64))
)]
pub(crate) struct CachePadded<T>(pub(crate) T);

/// Returns the contents of a `Option<T>`'s `Some(T)`, otherwise it returns
/// early from the function. Can alternatively have an `else` branch, or an
/// alternative "early return" statement, like `break` or `continue` for loops.
macro_rules! ward {
    ($o:expr) => (ward!($o, else { return; }));
    ($o:expr, else $body:block) => { if let Some(x) = $o { x } else { $body }; };
    ($o:expr, $early:stmt) => (ward!($o, else { $early }));
}

pub(crate) struct AlternateForm<T>(pub T);

impl<T: fmt::Display> fmt::Display for AlternateForm<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:#}", self.0)
    }
}

#[test]
fn alternate_form() {
    let cause = anyhow::anyhow!("inner");
    let error = cause.context("outer");
    assert_eq!(format!("{}", AlternateForm(error)), "outer: inner");
}
