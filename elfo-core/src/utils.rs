use std::{error::Error, fmt};

use derive_more::{Constructor, Deref};

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

#[derive(Constructor)]
pub(crate) struct ErrorChain<'a>(&'a dyn Error);

impl fmt::Display for ErrorChain<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)?;

        let mut cursor = self.0;
        while let Some(err) = cursor.source() {
            write!(f, ": {}", err)?;
            cursor = err;
        }

        Ok(())
    }
}

#[test]
fn trivial_error_chain() {
    let error = anyhow::anyhow!("oops");
    assert_eq!(format!("{}", ErrorChain(&*error)), "oops");
}

#[test]
fn error_chain() {
    let innermost = anyhow::anyhow!("innermost");
    let inner = innermost.context("inner");
    let outer = inner.context("outer");
    assert_eq!(
        format!("{}", ErrorChain(&*outer)),
        "outer: inner: innermost"
    );
}
