//! Bounded, crash-recoverable concatenated-zstd archive files.

use std::{
    collections::HashMap,
    fs::{File, OpenOptions},
    io::{Read as _, Seek as _, SeekFrom, Write as _},
    os::unix::{fs::OpenOptionsExt as _, prelude::PermissionsExt as _},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use sha2::{Digest as _, Sha256};

use crate::{
    journal_types::{JournalStreamId, ReceptionDay, ValidatedJournalBatch},
    store::{ArchiveFrame, FrameBoundary},
};

// 4,096 admitted targets × 32 typed streams × (30 retained days + one directory),
// plus one target directory each. Recovery may visit the full supported scale.
const MAX_ARCHIVE_FILES: usize = 4_067_328;
const MAX_JSONL_BATCH_BYTES: usize = 768 * 1024;
const MAX_COMPRESSED_FRAME_BYTES: usize = 1024 * 1024;

/// Sanitized archive failure.
#[derive(Debug, thiserror::Error)]
pub(crate) enum ArchiveError {
    #[error("safe-journal archive I/O failed")]
    Io(#[from] std::io::Error),
    #[error("safe-journal archive is malformed")]
    Invalid,
    #[error("safe-journal archive limit reached")]
    Limit,
    #[error("safe-journal archive capacity reached")]
    Capacity,
    #[error("safe-journal archive admission is poisoned")]
    Poisoned,
    #[error("safe-journal archive lost committed bytes")]
    MissingCommitted,
    #[error("safe-journal archive committed frame hash mismatch")]
    Hash,
    #[error("safe-journal archive mutation lock poisoned")]
    LockPoisoned,
    #[cfg(test)]
    #[error("injected archive sync failure")]
    Injected,
}

/// Filesystem archive with a process-wide capacity bound.
#[derive(Clone)]
pub(crate) struct JournalArchive {
    root: PathBuf,
    quota_bytes: u64,
    state: Arc<Mutex<ArchiveState>>,
    #[cfg(test)]
    append_hook: Option<Arc<TestAppendHook>>,
}

struct ArchiveState {
    used_bytes: u64,
    append_poisoned: bool,
}

#[cfg(test)]
pub(crate) struct TestAppendHook {
    pub(crate) entered: std::sync::Barrier,
    pub(crate) release: std::sync::Barrier,
    pub(crate) fail_after_write: std::sync::atomic::AtomicBool,
}

impl JournalArchive {
    /// Initialize the archive root with private permissions.
    pub(crate) fn open(data_root: &Path, quota_bytes: u64) -> Result<Self, ArchiveError> {
        if quota_bytes == 0 {
            return Err(ArchiveError::Invalid);
        }
        let root = data_root.join("logs");
        create_private_directory(&root)?;
        sync_directory(data_root)?;
        let archive = Self {
            root,
            quota_bytes,
            state: Arc::new(Mutex::new(ArchiveState {
                used_bytes: 0,
                append_poisoned: false,
            })),
            #[cfg(test)]
            append_hook: None,
        };
        archive
            .state
            .lock()
            .map_err(|_| ArchiveError::LockPoisoned)?
            .used_bytes = archive.scan_used_bytes()?;
        Ok(archive)
    }

    /// Validate committed tails and remove all bytes beyond SQLite's boundaries.
    pub(crate) fn recover(&self, boundaries: Vec<FrameBoundary>) -> Result<(), ArchiveError> {
        let mut state = self.state.lock().map_err(|_| ArchiveError::LockPoisoned)?;
        self.recover_locked(boundaries)?;
        state.used_bytes = self.scan_used_bytes()?;
        state.append_poisoned = false;
        Ok(())
    }

    /// Remove reception-day files older than a ledger-committed cutoff.
    pub(crate) fn prune_before(&self, cutoff: &ReceptionDay) -> Result<(), ArchiveError> {
        let mut state = self.state.lock().map_err(|_| ArchiveError::LockPoisoned)?;
        let mut visited = 0usize;
        for stream in std::fs::read_dir(&self.root)? {
            let stream = stream?;
            visited = visited.checked_add(1).ok_or(ArchiveError::Limit)?;
            let _stream_id = JournalStreamId::parse(
                stream
                    .file_name()
                    .into_string()
                    .map_err(|_| ArchiveError::Invalid)?,
            )
            .map_err(|_| ArchiveError::Invalid)?;
            if visited > MAX_ARCHIVE_FILES || !stream.file_type()?.is_dir() {
                return Err(ArchiveError::Limit);
            }
            for file in std::fs::read_dir(stream.path())? {
                let file = file?;
                visited = visited.checked_add(1).ok_or(ArchiveError::Limit)?;
                if visited > MAX_ARCHIVE_FILES || !file.file_type()?.is_file() {
                    return Err(ArchiveError::Limit);
                }
                let name = file
                    .file_name()
                    .into_string()
                    .map_err(|_| ArchiveError::Invalid)?;
                let day = ReceptionDay::parse(
                    name.strip_suffix(".jsonl.zst")
                        .ok_or(ArchiveError::Invalid)?
                        .to_owned(),
                )
                .map_err(|_| ArchiveError::Invalid)?;
                if day < *cutoff {
                    let bytes = file.metadata()?.len();
                    std::fs::remove_file(file.path())?;
                    sync_directory(&stream.path())?;
                    state.used_bytes = state
                        .used_bytes
                        .checked_sub(bytes)
                        .ok_or(ArchiveError::Invalid)?;
                }
            }
            if std::fs::read_dir(stream.path())?.next().is_none() {
                std::fs::remove_dir(stream.path())?;
                sync_directory(&self.root)?;
            }
        }
        Ok(())
    }

    fn recover_locked(&self, boundaries: Vec<FrameBoundary>) -> Result<(), ArchiveError> {
        if boundaries.len() > MAX_ARCHIVE_FILES {
            return Err(ArchiveError::Limit);
        }
        let mut committed = HashMap::new();
        for boundary in boundaries {
            committed.insert((boundary.stream_id.clone(), boundary.day.clone()), boundary);
        }
        let mut visited = 0usize;
        for stream in std::fs::read_dir(&self.root)? {
            let stream = stream?;
            visited = visited.checked_add(1).ok_or(ArchiveError::Limit)?;
            if visited > MAX_ARCHIVE_FILES {
                return Err(ArchiveError::Limit);
            }
            let stream_id = JournalStreamId::parse(
                stream
                    .file_name()
                    .into_string()
                    .map_err(|_| ArchiveError::Invalid)?,
            )
            .map_err(|_| ArchiveError::Invalid)?;
            if !stream.file_type()?.is_dir() {
                return Err(ArchiveError::Invalid);
            }
            for file in std::fs::read_dir(stream.path())? {
                let file = file?;
                visited = visited.checked_add(1).ok_or(ArchiveError::Limit)?;
                if visited > MAX_ARCHIVE_FILES || !file.file_type()?.is_file() {
                    return Err(ArchiveError::Limit);
                }
                let name = file
                    .file_name()
                    .into_string()
                    .map_err(|_| ArchiveError::Invalid)?;
                let day = ReceptionDay::parse(
                    name.strip_suffix(".jsonl.zst")
                        .ok_or(ArchiveError::Invalid)?
                        .to_owned(),
                )
                .map_err(|_| ArchiveError::Invalid)?;
                let path = file.path();
                if let Some(boundary) = committed.remove(&(stream_id.clone(), day)) {
                    verify_and_truncate(&path, &boundary)?;
                } else {
                    std::fs::remove_file(path)?;
                    sync_directory(&stream.path())?;
                }
            }
            if std::fs::read_dir(stream.path())?.next().is_none() {
                std::fs::remove_dir(stream.path())?;
                sync_directory(&self.root)?;
            }
        }
        if !committed.is_empty() {
            return Err(ArchiveError::MissingCommitted);
        }
        Ok(())
    }

    /// Append and sync one independent frame, returning its durable boundary.
    pub(crate) fn append(
        &self,
        stream_id: &JournalStreamId,
        day: &ReceptionDay,
        batch: &ValidatedJournalBatch,
    ) -> Result<ArchiveFrame, ArchiveError> {
        let jsonl = batch.jsonl();
        if jsonl.is_empty() || jsonl.len() > MAX_JSONL_BATCH_BYTES {
            return Err(ArchiveError::Invalid);
        }
        let frame = zstd::stream::encode_all(jsonl, 3).map_err(ArchiveError::Io)?;
        let mut state = self.state.lock().map_err(|_| ArchiveError::LockPoisoned)?;
        let stream_id = stream_id.as_str();
        let day_text = day.as_str();
        #[cfg(test)]
        if let Some(hook) = &self.append_hook {
            hook.entered.wait();
            hook.release.wait();
        }
        if state.append_poisoned {
            return Err(ArchiveError::Poisoned);
        }
        if frame.len() > MAX_COMPRESSED_FRAME_BYTES {
            return Err(ArchiveError::Capacity);
        }
        let next_used_bytes = state
            .used_bytes
            .checked_add(frame.len() as u64)
            .ok_or(ArchiveError::Limit)?;
        if next_used_bytes > self.quota_bytes {
            return Err(ArchiveError::Capacity);
        }
        state.used_bytes = next_used_bytes;
        let result = self.append_reserved(stream_id, day_text, day, &frame);
        if result.is_err() {
            state.append_poisoned = true;
        }
        result
    }

    fn append_reserved(
        &self,
        stream_id: &str,
        day_text: &str,
        day: &ReceptionDay,
        frame: &[u8],
    ) -> Result<ArchiveFrame, ArchiveError> {
        let stream_dir = self.root.join(stream_id);
        if !stream_dir.exists() {
            create_private_directory(&stream_dir)?;
            sync_directory(&self.root)?;
        }
        let path = stream_dir.join(format!("{day_text}.jsonl.zst"));
        let existed = path.exists();
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .read(true)
            .mode(0o600)
            .open(&path)?;
        let start = file.metadata()?.len();
        file.write_all(frame)?;
        #[cfg(test)]
        if self.append_hook.as_ref().is_some_and(|hook| {
            hook.fail_after_write
                .swap(false, std::sync::atomic::Ordering::SeqCst)
        }) {
            return Err(ArchiveError::Injected);
        }
        file.sync_data()?;
        if !existed {
            sync_directory(&stream_dir)?;
        }
        let end = start
            .checked_add(frame.len() as u64)
            .ok_or(ArchiveError::Limit)?;
        Ok(ArchiveFrame {
            day: day.clone(),
            start,
            end,
            hash: Sha256::digest(frame).into(),
        })
    }

    /// Remove a just-appended frame after its SQLite CAS lost.
    pub(crate) fn truncate_uncommitted(
        &self,
        stream_id: &JournalStreamId,
        frame: &ArchiveFrame,
    ) -> Result<(), ArchiveError> {
        let mut state = self.state.lock().map_err(|_| ArchiveError::LockPoisoned)?;
        let file = OpenOptions::new()
            .write(true)
            .open(self.path(stream_id, &frame.day))?;
        file.set_len(frame.start)?;
        file.sync_data()?;
        state.used_bytes = state
            .used_bytes
            .checked_sub(frame.end - frame.start)
            .ok_or(ArchiveError::Invalid)?;
        Ok(())
    }

    fn path(&self, stream_id: &JournalStreamId, day: &ReceptionDay) -> PathBuf {
        self.root
            .join(stream_id.as_str())
            .join(format!("{}.jsonl.zst", day.as_str()))
    }

    fn scan_used_bytes(&self) -> Result<u64, ArchiveError> {
        let mut total = 0u64;
        let mut entries = 0usize;
        for stream in std::fs::read_dir(&self.root)? {
            let stream = stream?;
            entries += 1;
            if entries > MAX_ARCHIVE_FILES || !stream.file_type()?.is_dir() {
                return Err(ArchiveError::Limit);
            }
            for file in std::fs::read_dir(stream.path())? {
                let file = file?;
                entries += 1;
                if entries > MAX_ARCHIVE_FILES || !file.file_type()?.is_file() {
                    return Err(ArchiveError::Limit);
                }
                total = total
                    .checked_add(file.metadata()?.len())
                    .ok_or(ArchiveError::Limit)?;
            }
        }
        Ok(total)
    }

    #[cfg(test)]
    fn used_bytes(&self) -> Result<u64, ArchiveError> {
        Ok(self
            .state
            .lock()
            .map_err(|_| ArchiveError::LockPoisoned)?
            .used_bytes)
    }
}

#[cfg(test)]
impl JournalArchive {
    pub(crate) fn with_append_hook(mut self, hook: Arc<TestAppendHook>) -> Self {
        self.append_hook = Some(hook);
        self
    }
}

fn verify_and_truncate(path: &Path, boundary: &FrameBoundary) -> Result<(), ArchiveError> {
    if boundary.start >= boundary.end
        || boundary.end - boundary.start > MAX_COMPRESSED_FRAME_BYTES as u64
    {
        return Err(ArchiveError::Invalid);
    }
    let mut file = OpenOptions::new().read(true).write(true).open(path)?;
    if file.metadata()?.len() < boundary.end {
        return Err(ArchiveError::MissingCommitted);
    }
    file.seek(SeekFrom::Start(boundary.start))?;
    let mut bytes = vec![0; (boundary.end - boundary.start) as usize];
    file.read_exact(&mut bytes)?;
    if <[u8; 32]>::from(Sha256::digest(bytes)) != boundary.hash {
        return Err(ArchiveError::Hash);
    }
    file.set_len(boundary.end)?;
    file.sync_data()?;
    Ok(())
}

fn create_private_directory(path: &Path) -> Result<(), ArchiveError> {
    match std::fs::create_dir(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.into()),
    }
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.file_type().is_dir() {
        return Err(ArchiveError::Invalid);
    }
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    Ok(())
}

fn sync_directory(path: &Path) -> Result<(), ArchiveError> {
    File::open(path)?.sync_all()?;
    Ok(())
}

#[cfg(test)]
#[path = "archive_tests.rs"]
mod tests;
