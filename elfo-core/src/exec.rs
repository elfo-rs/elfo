use std::{error::Error, future::Future};

use crate::sealed::Sealed;

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

pub trait ExecResult: Sealed {
    fn unify(self) -> Result<(), BoxedError>;
}

impl ExecResult for () {
    fn unify(self) -> Result<(), BoxedError> {
        Ok(())
    }
}

impl<E: Into<BoxedError>> Sealed for Result<(), E> {}

impl<E> ExecResult for Result<(), E>
where
    E: Into<BoxedError>,
{
    fn unify(self) -> Result<(), BoxedError> {
        self.map_err(Into::into)
    }
}
