#![doc(hidden)] // TODO: "partially" unstable for now

use std::{future::Future, io};

use tokio::{runtime::Handle, task::JoinHandle};

use crate::scope::{self, Scope};

/// A factory which is used to configure the properties of a new task.
///
/// This is a stable wrapper over [`tokio::task::Builder`].
///
/// Features:
/// * Spawning tasks inside a given scope (or the current scope by default). It
///   means that logs/metrics/dumps will be associated with specific actor, that
///   is especially useful for blocking tasks (e.g. for I/O operations).
/// * If compiled with the `tokio_unstable` feature, it will set the task name
///   in order to make it easier to identify tasks in `tokio-console`.
///
/// Note: this is an unstable API.
///
/// [`tokio::task::Builder`]: https://docs.rs/tokio/latest/tokio/task/struct.Builder.html
pub struct Builder {
    scope: Scope,
}

impl Default for Builder {
    fn default() -> Self {
        Self::new(scope::expose())
    }
}

// NOTE: all spawning methods should be marked with the `track_caller` attribute
//       to show better locations in `tokio-console` listings.

impl Builder {
    /// Creates a new task builder with the given scope instead of the current
    /// one used by default in `Builder::default()`.
    pub fn new(scope: Scope) -> Self {
        Self { scope }
    }

    /// Spawns a task with this builder’s settings on the current runtime.
    ///
    /// # Panics
    ///
    /// Panics if not called from the context of a Tokio runtime.
    #[track_caller]
    pub fn spawn<F>(self, f: F) -> io::Result<JoinHandle<F::Output>>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        self.spawn_on(f, &Handle::current())
    }

    // Spawns a task with this builder’s settings on the provided runtime handle.
    #[track_caller]
    pub fn spawn_on<F>(self, f: F, handle: &Handle) -> io::Result<JoinHandle<F::Output>>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        #[cfg(not(tokio_unstable))]
        return Ok(handle.spawn(self.scope.within(f)));

        #[cfg(tokio_unstable)]
        tokio::task::Builder::new()
            .name(&self.scope.meta().to_string())
            .spawn_on(self.scope.within(f), handle)
    }

    /// Spawns blocking code on the current runtime’s blocking threadpool.
    ///
    /// # Panics
    ///
    /// Panics if not called from the context of a Tokio runtime.
    #[track_caller]
    pub fn spawn_blocking<F, R>(self, f: F) -> io::Result<JoinHandle<R>>
    where
        F: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        self.spawn_blocking_on(f, &Handle::current())
    }

    /// Spawns blocking code on the provided runtime’s blocking threadpool.
    #[track_caller]
    pub fn spawn_blocking_on<F, R>(self, f: F, handle: &Handle) -> io::Result<JoinHandle<R>>
    where
        F: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        #[cfg(not(tokio_unstable))]
        return Ok(handle.spawn_blocking(|| self.scope.sync_within(f)));

        #[cfg(tokio_unstable)]
        tokio::task::Builder::new()
            .name(&self.scope.meta().to_string())
            .spawn_blocking_on(|| self.scope.sync_within(f), handle)
    }
}

/// Spawns blocking code on the current runtime’s blocking threadpool.
///
/// See `Builder` for more details.
///
/// # Panics
///
/// Panics if not called from the context of a Tokio runtime.
#[track_caller]
pub fn spawn_blocking<F, R>(f: F) -> JoinHandle<R>
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    Builder::default()
        .spawn_blocking(f)
        .expect("spawn a blocking task")
}
