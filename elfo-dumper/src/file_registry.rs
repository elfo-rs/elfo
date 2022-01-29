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
    pub(crate) async fn open(&self, path: &str, force: bool) -> Result<()> {
        let mut file = {
            let mut files = self.files.lock();

            if !files.contains_key(path) {
                files.insert(path.to_string(), FileHandle::default());
            }

            files.get(path).unwrap().clone()
        };

        if file.open(path, force).await? {
            debug!(%path, "file opened");
        }

        Ok(())
    }

    pub(crate) async fn acquire(&self, path: &str) -> FileHandle {
        self.files
            .lock()
            .get(path)
            .expect("file must be open already")
            .clone()
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
