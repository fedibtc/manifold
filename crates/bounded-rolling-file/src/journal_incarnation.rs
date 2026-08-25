//! Atomic publication and validation of journal storage-generation identity.

use std::io::{self, Read as _, Write as _};

use uuid::Version;

use crate::anchored_directory::{AnchoredDirectory, validate_single_link};
use crate::{INCARNATION_FILE, INCARNATION_PENDING_FILE, INCARNATION_TEXT_BYTES};

/// Durable identity of one journal storage generation.
#[derive(Clone, Eq, PartialEq)]
pub struct JournalIncarnation(String);

impl JournalIncarnation {
    /// Return the canonical lowercase UUIDv7 text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl core::fmt::Debug for JournalIncarnation {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str("JournalIncarnation([REDACTED])")
    }
}
/// Open or initialize identity while the caller holds `incarnation.lock`.
///
/// Successful return means final-name publication is directory-synced.
pub(super) fn open_incarnation_locked(
    directory: &AnchoredDirectory,
) -> io::Result<JournalIncarnation> {
    match read_incarnation(directory, INCARNATION_FILE) {
        Ok(incarnation) => {
            let pending_exists = entry_exists(directory, INCARNATION_PENDING_FILE)?;
            validate_published_incarnation_links(directory, pending_exists)?;
            if pending_exists {
                // Pending is evidence that final-name publication may not have
                // been committed. Commit it before removing the evidence.
                directory.sync()?;
                directory.unlink(INCARNATION_PENDING_FILE)?;
                directory.sync()?;
            }
            return Ok(incarnation);
        }
        Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
        Err(_) => {}
    }

    let incarnation = match read_incarnation(directory, INCARNATION_PENDING_FILE) {
        Ok(incarnation) => {
            let pending = directory.open_file(
                INCARNATION_PENDING_FILE,
                rustix::fs::OFlags::RDONLY,
                rustix::fs::Mode::empty(),
            )?;
            validate_single_link(&pending)?;
            pending.sync_all()?;
            incarnation
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let incarnation = JournalIncarnation(uuid::Uuid::now_v7().to_string());
            write_pending(directory, &incarnation)?;
            incarnation
        }
        Err(error) if error.kind() == io::ErrorKind::InvalidData => {
            directory.unlink(INCARNATION_PENDING_FILE)?;
            directory.sync()?;
            let incarnation = JournalIncarnation(uuid::Uuid::now_v7().to_string());
            write_pending(directory, &incarnation)?;
            incarnation
        }
        Err(error) => return Err(error),
    };

    match directory.link(INCARNATION_PENDING_FILE, INCARNATION_FILE) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            let published = read_incarnation(directory, INCARNATION_FILE)?;
            directory.sync()?;
            directory.unlink(INCARNATION_PENDING_FILE)?;
            directory.sync()?;
            return Ok(published);
        }
        Err(error) => return Err(error),
    }
    directory.sync()?;
    directory.unlink(INCARNATION_PENDING_FILE)?;
    directory.sync()?;
    Ok(incarnation)
}

fn write_pending(
    directory: &AnchoredDirectory,
    incarnation: &JournalIncarnation,
) -> io::Result<()> {
    let mut file = directory.open_file(
        INCARNATION_PENDING_FILE,
        rustix::fs::OFlags::CREATE | rustix::fs::OFlags::EXCL | rustix::fs::OFlags::WRONLY,
        rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
    )?;
    writeln!(file, "{}", incarnation.as_str())?;
    file.sync_all()
}

/// Read and validate one descriptor-relative canonical UUIDv7 metadata entry.
pub(super) fn read_incarnation(
    directory: &AnchoredDirectory,
    name: &str,
) -> io::Result<JournalIncarnation> {
    let file = directory.open_file(name, rustix::fs::OFlags::RDONLY, rustix::fs::Mode::empty())?;
    let mut bytes = Vec::with_capacity(INCARNATION_TEXT_BYTES);
    file.take((INCARNATION_TEXT_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() != INCARNATION_TEXT_BYTES || bytes.last() != Some(&b'\n') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "journal incarnation metadata is malformed",
        ));
    }
    let text = std::str::from_utf8(&bytes[..INCARNATION_TEXT_BYTES - 1]).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "journal incarnation metadata is malformed",
        )
    })?;
    let uuid = uuid::Uuid::parse_str(text).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "journal incarnation metadata is malformed",
        )
    })?;
    if uuid.get_version() != Some(Version::SortRand) || uuid.to_string() != text {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "journal incarnation metadata is malformed",
        ));
    }
    Ok(JournalIncarnation(text.to_owned()))
}

fn entry_exists(directory: &AnchoredDirectory, name: &str) -> io::Result<bool> {
    match directory.open_file(name, rustix::fs::OFlags::RDONLY, rustix::fs::Mode::empty()) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

fn validate_published_incarnation_links(
    directory: &AnchoredDirectory,
    pending_exists: bool,
) -> io::Result<()> {
    let published = directory.open_file(
        INCARNATION_FILE,
        rustix::fs::OFlags::RDONLY,
        rustix::fs::Mode::empty(),
    )?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        let published_metadata = published.metadata()?;
        if pending_exists {
            let pending = directory.open_file(
                INCARNATION_PENDING_FILE,
                rustix::fs::OFlags::RDONLY,
                rustix::fs::Mode::empty(),
            )?;
            let pending_metadata = pending.metadata()?;
            if published_metadata.nlink() != 2
                || pending_metadata.nlink() != 2
                || published_metadata.dev() != pending_metadata.dev()
                || published_metadata.ino() != pending_metadata.ino()
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "journal incarnation publication links are inconsistent",
                ));
            }
        } else {
            validate_single_link(&published)?;
        }
    }
    Ok(())
}
