use derive_more::{Display, Error};

#[derive(Debug, Display, Error)]
#[non_exhaustive]
pub enum StartError {
    /// Configs must be valid at the start-up.
    #[display(fmt = "invalid config")]
    InvalidConfig,
    Other(#[error(not(source))] String),
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
