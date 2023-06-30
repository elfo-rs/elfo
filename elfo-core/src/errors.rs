use std::{
    collections::BTreeMap,
    fmt::{Debug, Display},
};

use derive_more::{Display, Error};

#[derive(Error)]
#[non_exhaustive]
pub struct StartError {
    pub errors: Vec<StartGroupError>,
}

impl StartError {
    pub(crate) fn single(group: String, reason: String) -> Self {
        Self {
            errors: vec![StartGroupError { group, reason }],
        }
    }

    pub(crate) fn multiple(errors: Vec<StartGroupError>) -> Self {
        Self { errors }
    }
}

fn group_errors(errors: Vec<StartGroupError>) -> BTreeMap<String, Vec<String>> {
    // `BTreeMap` is used purely to provide consistent group order in error
    // messages.
    let mut group_errors = BTreeMap::<String, Vec<String>>::new();
    for error in errors {
        group_errors
            .entry(error.group)
            .or_default()
            .push(error.reason);
    }
    // Sort errors to provide consistent order.
    for errors in group_errors.values_mut() {
        errors.sort();
    }
    group_errors
}

impl Debug for StartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if f.alternate() {
            let mut s = f.debug_struct("StartError");
            s.field("errors", &self.errors);
            s.finish()
        } else {
            write!(f, "failed to start, see errors by actor groups\n\n")?;
            for (group, errors) in group_errors(self.errors.clone()) {
                writeln!(f, "{group}:")?;
                for error in errors {
                    writeln!(f, "- {error}")?;
                }
                writeln!(f)?;
            }
            Ok(())
        }
    }
}

impl Display for StartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "failed to start, see errors: ")?;
        let mut i = 1;
        for (group, errors) in group_errors(self.errors.clone()) {
            for error in errors {
                write!(f, "{i}. {error} ({group}); ")?;
                i += 1;
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Display, Error)]
#[non_exhaustive]
#[display(fmt = "error from group {group}: {reason}")]
pub struct StartGroupError {
    pub group: String,
    pub reason: String,
}

#[derive(Debug, Display, Error)]
#[display(fmt = "mailbox closed")]
pub struct SendError<T>(#[error(not(source))] pub T);

#[derive(Debug, Display, Error)]
pub enum TrySendError<T> {
    /// The mailbox is full.
    #[display(fmt = "mailbox full")]
    Full(#[error(not(source))] T),
    /// The mailbox has been closed.
    #[display(fmt = "mailbox closed")]
    Closed(#[error(not(source))] T),
}

impl<T> TrySendError<T> {
    /// Converts the error into its inner value.
    #[inline]
    pub fn into_inner(self) -> T {
        match self {
            Self::Closed(inner) => inner,
            Self::Full(inner) => inner,
        }
    }

    /// Transforms the inner message.
    #[inline]
    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> TrySendError<U> {
        match self {
            Self::Full(inner) => TrySendError::Full(f(inner)),
            Self::Closed(inner) => TrySendError::Closed(f(inner)),
        }
    }

    /// Returns whether the error is the `Full` variant.
    #[inline]
    pub fn is_full(&self) -> bool {
        matches!(self, Self::Full(_))
    }

    /// Returns whether the error is the `Closed` variant.
    #[inline]
    pub fn is_closed(&self) -> bool {
        matches!(self, Self::Closed(_))
    }
}

#[derive(Debug, Display, Error)]
pub enum RequestError<T> {
    // Nobody has responded to the request.
    #[display(fmt = "request ignored")]
    Ignored, // TODO: can we provide `T` here?
    /// The mailbox has been closed.
    #[display(fmt = "mailbox closed")]
    Closed(#[error(not(source))] T),
}

impl<T> RequestError<T> {
    /// Converts the error into its inner value.
    #[inline]
    pub fn into_inner(self) -> Option<T> {
        match self {
            Self::Ignored => None,
            Self::Closed(inner) => Some(inner),
        }
    }

    /// Transforms the inner message.
    #[inline]
    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> RequestError<U> {
        match self {
            Self::Ignored => RequestError::Ignored,
            Self::Closed(inner) => RequestError::Closed(f(inner)),
        }
    }

    /// Returns whether the error is the `Full` variant.
    #[inline]
    pub fn is_ignored(&self) -> bool {
        matches!(self, Self::Ignored)
    }

    /// Returns whether the error is the `Closed` variant.
    #[inline]
    pub fn is_closed(&self) -> bool {
        matches!(self, Self::Closed(_))
    }
}

#[derive(Debug, Clone, Display, Error)]
pub enum TryRecvError {
    /// The mailbox is empty.
    #[display(fmt = "mailbox empty")]
    Empty,
    /// The mailbox has been closed.
    #[display(fmt = "mailbox closed")]
    Closed,
}

impl TryRecvError {
    /// Returns whether the error is the `Empty` variant.
    #[inline]
    pub fn is_empty(&self) -> bool {
        matches!(self, Self::Empty)
    }

    /// Returns whether the error is the `Closed` variant.
    #[inline]
    pub fn is_closed(&self) -> bool {
        matches!(self, Self::Closed)
    }
}
