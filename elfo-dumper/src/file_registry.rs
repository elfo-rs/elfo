use std::{fs::File, io::Write, sync::Arc};

use eyre::{eyre, Result};
use fxhash::FxHashMap;
use parking_lot::Mutex;
use tokio::{
    fs::{File as AsyncFile, OpenOptions as AsyncOpenOptions},
    sync::Mutex as AsyncMutex,
};
use tracing::debug;

// === FileRegistry ===

#[derive(Default)]
pub(crate) struct FileRegistry {
    files: Mutex<FxHashMap<String, FileHandle>>,
}

impl FileRegistry {
    fn insert_handle(path: &str, files: &mut FxHashMap<String, FileHandle>) -> FileHandle {
        let contained = files.contains_key(path);
        if !contained {
            files.insert(path.to_string(), FileHandle::default());
        }

        files.get(path).unwrap().clone()
    }

    async fn open_inner(path: &str, force: bool, fh: &mut FileHandle) -> Result<()> {
        if fh.open(path, force).await? {
            debug!(%path, "file opened");
        }

        Ok(())
    }

    pub(crate) async fn open(&self, path: &str, force: bool) -> Result<()> {
        let mut file = {
            let mut files = self.files.lock();
            Self::insert_handle(path, &mut *files)
        };

        Self::open_inner(path, force, &mut file).await?;
        Ok(())
    }

    /// Open file for writing dump in it, closing previous one if
    /// paths differ.
    pub(crate) async fn acquire_for_write(&self, old_path: &str, path: &str) -> Result<FileHandle> {
        let mut fh = {
            let mut files = self.files.lock();
            if old_path != path {
                // Close previous file.
                // NOTE: this won't cause any bugs if two dumpers write to
                // the same file, since `FileHandle` is reference counted, thus
                // removing it from map only decreases refcounter by one.
                files.remove(old_path);
            }

            Self::insert_handle(path, &mut *files)
        };

        // NOTE: a bit racy part here, two threads observe
        // file status (open or not open) in random order, but it doesn't
        // matter here, since after first thread opens the file - it's guaranteed by
        // underlying mutex that other threads will see it as open, thus opening
        // will pe performed only once.
        Self::open_inner(path, false, &mut fh).await?;
        Ok(fh)
    }

    pub(crate) async fn sync(&self, path: &str) -> Result<()> {
        let file = self
            .files
            .lock()
            .get(path)
            .expect("file must be open already")
            .clone();

        file.sync().await?;
        debug!(%path, "file synchronized");
        Ok(())
    }
}

// === FileHandle ===

#[derive(Default, Clone)]
pub(crate) struct FileHandle {
    file: Arc<AsyncMutex<Option<File>>>,
}

impl FileHandle {
    async fn open(&mut self, path: &str, force: bool) -> Result<bool> {
        let mut file_lock = self.file.lock().await;

        if file_lock.is_some() && !force {
            return Ok(false);
        }

        let file = AsyncOpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .await?
            .into_std()
            .await;

        *file_lock = Some(file);
        Ok(true)
    }

    /// Must be called in a blocking context (e.g. inside `spawn_blocking`).
    pub(crate) fn write(&self, buffer: &[u8]) -> Result<()> {
        let mut file_lock = self.file.blocking_lock();
        let mut file = file_lock
            .take()
            .ok_or_else(|| eyre!("file handle is poisoned"))?;
        file.write_all(buffer)?;
        file.flush()?; // on all (?) OS does nothing
        *file_lock = Some(file);
        Ok(())
    }

    pub(crate) async fn sync(&self) -> Result<()> {
        let mut file_lock = self.file.lock().await;
        let file = file_lock
            .take()
            .ok_or_else(|| eyre!("file handle is poisoned"))?;
        let file = AsyncFile::from_std(file);
        file.sync_all().await?;
        *file_lock = Some(file.into_std().await);
        Ok(())
    }
}
