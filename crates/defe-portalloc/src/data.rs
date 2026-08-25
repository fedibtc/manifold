mod dto;

use std::fs;
use std::path::PathBuf;

use anyhow::{Result, bail};
use fs2::FileExt as _;

use crate::util;

/// Root directory where the allocator keeps lock and data files.
pub(crate) struct DataDir {
    /// Directory containing the allocator state.
    path: PathBuf,
    /// Advisory lock file shared by cooperating processes.
    lock_file: fs::File,
}

impl DataDir {
    /// Open the allocator data directory, creating it if needed.
    pub(crate) fn new(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        ensure_root_exists(&path)?;
        let lock_file = util::open_lock_file(&path)?;

        Ok(Self { path, lock_file })
    }

    /// Acquire the advisory lock for the duration of `f`.
    pub(crate) fn with_lock<T>(
        &mut self,
        f: impl FnOnce(&mut LockedRoot<'_>) -> Result<T>,
    ) -> Result<T> {
        f(&mut LockedRoot::new(&self.path, &mut self.lock_file)?)
    }
}

fn ensure_root_exists(dir: &PathBuf) -> Result<()> {
    if !dir.try_exists()? {
        fs::create_dir_all(dir)?;
    }
    Ok(())
}

/// A locked handle to the allocator root directory.
pub(crate) struct LockedRoot<'a> {
    /// Directory containing the allocator state.
    path: &'a PathBuf,
    /// File holding the advisory lock.
    lock_file: &'a mut fs::File,
    /// Whether this handle currently owns the lock.
    locked: bool,
}

impl Drop for LockedRoot<'_> {
    fn drop(&mut self) {
        if self.locked {
            let _ = self.lock_file.unlock();
            self.locked = false;
        }
    }
}

impl<'a> LockedRoot<'a> {
    fn new(path: &'a PathBuf, lock_file: &'a mut fs::File) -> Result<Self> {
        let mut locked_root = Self {
            path,
            lock_file,
            locked: false,
        };
        locked_root.lock()?;
        Ok(locked_root)
    }

    fn lock(&mut self) -> Result<()> {
        if self.lock_file.try_lock_exclusive().is_err() {
            self.lock_file.lock_exclusive()?;
        }
        self.locked = true;
        Ok(())
    }

    fn data_file_path(&self) -> PathBuf {
        self.path.join("defe-portalloc.json")
    }

    fn ensure_locked(&self) -> anyhow::Result<()> {
        if !self.locked {
            bail!("LockedRoot no longer valid");
        }
        Ok(())
    }

    /// Load the allocator state from disk, or return default empty state.
    pub(crate) fn load_data(&self) -> Result<dto::RootData> {
        self.ensure_locked()?;
        let path = self.data_file_path();
        if !path.try_exists()? {
            return Ok(Default::default());
        }
        Ok(serde_json::from_reader::<_, _>(fs::File::open(path)?)?)
    }

    /// Store allocator state to disk atomically.
    pub(crate) fn store_data(&mut self, data: &dto::RootData) -> Result<()> {
        self.ensure_locked()?;
        util::store_json_pretty_to_file(&self.data_file_path(), data)
    }
}
