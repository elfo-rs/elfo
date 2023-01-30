use std::{error::Error, fmt};

pub(crate) struct ErrorChain<'a>(pub(crate) &'a dyn Error);

impl fmt::Display for ErrorChain<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)?;

        let mut cursor = self.0;
        while let Some(err) = cursor.source() {
            write!(f, ": {err}")?;
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
