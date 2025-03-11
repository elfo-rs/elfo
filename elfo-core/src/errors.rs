use std::{collections::BTreeMap, fmt::Debug};

use derive_more::{Display, Error, IsVariant};

// === StartError ===

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
            write!(f, "failed to start\n\n")?;
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
        write!(f, "failed to start: ")?;
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
#[display("error from group {group}: {reason}")]
pub struct StartGroupError {
    pub group: String,
    pub reason: String,
}

// === SendError ===

#[derive(Debug, Display, Error)]
#[display("mailbox closed")]
pub struct SendError<T>(#[error(not(source))] pub T);

impl<T> SendError<T> {
    #[inline]
    pub fn into_inner(self) -> T {
        self.0
    }

    /// Transforms the inner message.
    #[inline]
    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> SendError<U> {
        SendError(f(self.0))
    }
}

// === TrySendError ===

#[derive(Debug, Display, Error)]
pub enum TrySendError<T> {
    /// The mailbox is full.
    #[display("mailbox full")]
    Full(#[error(not(source))] T),
    /// The mailbox has been closed.
    #[display("mailbox closed")]
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

impl<T> From<SendError<T>> for TrySendError<T> {
    #[inline]
    fn from(err: SendError<T>) -> Self {
        TrySendError::Closed(err.0)
    }
}

// === RequestError ===

#[derive(Debug, IsVariant, Display, Clone, Error)]
pub enum RequestError {
    /// Receiver hasn't got the request.
    #[display("request failed")]
    Failed,
    /// Receiver has got the request, but ignored it.
    #[display("request ignored")]
    Ignored,
}

// === TryRecvError ===

#[derive(Debug, Clone, Display, Error)]
pub enum TryRecvError {
    /// The mailbox is empty.
    #[display("mailbox empty")]
    Empty,
    /// The mailbox has been closed.
    #[display("mailbox closed")]
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
