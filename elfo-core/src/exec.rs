use std::{error::Error, future::Future};

use sealed::sealed;

pub(crate) trait Exec<CTX>: Send + Sync + 'static {
    type Output: Future + Send + 'static;

    fn exec(&self, ctx: CTX) -> Self::Output;
}

pub(crate) type BoxedError = Box<dyn Error + 'static>;

impl<F, CTX, O, ER> Exec<CTX> for F
where
    F: Fn(CTX) -> O + Send + Sync + 'static,
    O: Future<Output = ER> + Send + 'static,
    ER: ExecResult,
{
    type Output = O;

    #[inline]
    fn exec(&self, ctx: CTX) -> O {
        self(ctx)
    }
}

#[sealed]
pub trait ExecResult {
    fn unify(self) -> Result<(), BoxedError>;
}

#[sealed]
impl ExecResult for () {
    fn unify(self) -> Result<(), BoxedError> {
        Ok(())
    }
}

#[sealed]
impl ExecResult for never::Never {
    #[allow(private_interfaces)]
    fn unify(self) -> Result<(), BoxedError> {
        self
    }
}

#[sealed]
impl<E> ExecResult for Result<(), E>
where
    E: Into<BoxedError>,
{
    fn unify(self) -> Result<(), BoxedError> {
        self.map_err(Into::into)
    }
}

// === Never (!) ===

mod never {
    pub(super) type Never = <F as HasOutput>::Output;

    pub(super) trait HasOutput {
        type Output;
    }

    impl<O> HasOutput for fn() -> O {
        type Output = O;
    }

    type F = fn() -> !;
}
