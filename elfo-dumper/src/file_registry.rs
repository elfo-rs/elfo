use std::{
    fs::{File, OpenOptions},
    io::Write,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Weak,
    },
};

use eyre::{Result, WrapErr};
use fxhash::FxHashMap;
use parking_lot::Mutex;
use tokio::{fs::File as AsyncFile, sync::Mutex as AsyncMutex};
use tracing::debug;

// === FileRegistry ===

#[derive(Default)]
pub(crate) struct FileRegistry {
    files: Mutex<FxHashMap<String, FileHandle>>,
}

impl FileRegistry {
    /// Returns a handle to the file, possibly shared among multiple actors.
    pub(crate) fn acquire(self: &Arc<Self>, path: &str) -> FileHandle {
        let mut files = self.files.lock();

        if let Some(file) = files.get(path) {
            return file.clone();
        }

        let handle = FileHandleInner {
            reopen: <_>::default(),
            file: AsyncMutex::new(MaybeFile::new(path)),
            registry: Arc::downgrade(self),
        };

        let handle = FileHandle(Arc::new(handle));
        files.insert(path.into(), handle.clone());
        handle
    }

    /// Forces reopening of files on the next write.
    pub(crate) fn schedule_reopen(&self) {
        for handle in self.files.lock().values() {
            handle.schedule_reopen();
        }
    }
}

// === FileHandle ===

#[derive(Clone)]
pub(crate) struct FileHandle(Arc<FileHandleInner>);

struct FileHandleInner {
    reopen: AtomicBool,
    file: AsyncMutex<MaybeFile>,
    registry: Weak<FileRegistry>,
}

impl FileHandle {
    /// Must be called in a blocking context (e.g. inside `spawn_blocking`).
    pub(crate) fn write(&self, buffer: &[u8]) -> Result<()> {
        let this = &self.0;
        let mut file_slot = this.file.blocking_lock();

        // TODO: reopen logic

        // Taking allows to force reopening on the next write in the case of errors.
        let mut file = file_slot
            .take()
            .map(Ok)
            .unwrap_or_else(|| open_file(&this.path))
            .wrap_err_with(|| format!("cannot open {:?}", this.path))?;

        file.write_all(buffer)
            .wrap_err_with(|| format!("cannot write to {:?}", this.path))?;

        file.flush() // probably does nothing
            .wrap_err_with(|| format!("cannot flush to {:?}", this.path))?;

        debug!(
            message = "written to file",
            path = %this.path,
            size = buffer.len(),
        );

        *file_slot = Some(file);
        Ok(())
    }

    pub(crate) async fn sync(&self) -> Result<()> {
        let this = &self.0;
        let mut file_lock = this.file.lock().await;

        // Taking allows to force reopening on the next write in the case of errors.
        let Some(file) = file_lock.take() else {
            return Ok(());
        };

        let file = AsyncFile::from_std(file);
        file.sync_all()
            .await
            .wrap_err_with(|| format!("cannot sync {:?}", this.path))?;
        *file_lock = Some(file.into_std().await);

        debug!(
            message = "file synchronized",
            path = %this.path,
        );

        Ok(())
    }

    fn schedule_reopen(&self) {
        self.0.reopen.store(true, Ordering::Relaxed);
    }
}

impl Drop for FileHandle {
    fn drop(&mut self) {
        if Arc::strong_count(&self.0) > 1 {
            return;
        }

        let this = &self.0;

        if let Some(registry) = this.registry.upgrade() {
            registry.files.lock().remove(&this.path);
        };

        // TODO: WRONG PLACE
        debug!(
            message = "file closed",
            path = %this.path,
        );

        // TODO: with async drop it can be reasonable to sync the file here
    }
}

struct MaybeFile {
    path: String,
    file: Option<File>,
}

impl MaybeFile {
    fn new(path: &str) -> Self {
        Self {
            path: path.into(),
            file: None,
        }
    }

    #[cold]
    #[inline(never)]
    fn open(&mut self, path: &str) -> Result<()> {
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        debug!(%path, "file opened");
        self.file = Some(file);
        Ok(())
    }
}

impl Drop for MaybeFile {
    fn drop(&mut self) {
        if self.file.is_some() {
            debug!(path = %self.path, "file closed");
        }
    }
}
