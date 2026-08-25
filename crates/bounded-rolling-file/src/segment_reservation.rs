//! Durable monotone segment-number reservation and crash-state recovery.

use std::collections::BTreeMap;
use std::io::{self, Read as _, Write as _};

use crate::anchored_directory::{AnchoredDirectory, validate_single_link};
use crate::{NEXT_SEGMENT_FILE, NEXT_SEGMENT_PENDING_FILE, SEGMENT_RESERVATION_MARKER_FILE};

/// Recover or initialize the next segment number before segment creation.
///
/// The caller serializes writers; success is durably ahead of every discovered
/// segment.
pub(super) fn open_next_segment(
    directory: &AnchoredDirectory,
    segments: &BTreeMap<u64, String>,
) -> io::Result<u64> {
    match read_next_segment_allow_publication_link(directory, NEXT_SEGMENT_FILE) {
        Ok(next) => {
            match read_next_segment_allow_publication_link(directory, NEXT_SEGMENT_PENDING_FILE) {
                Ok(pending) => {
                    sync_pending(directory)?;
                    let same_publication = pending == next
                        && same_entry(directory, NEXT_SEGMENT_FILE, NEXT_SEGMENT_PENDING_FILE)?;
                    if same_publication {
                        directory.sync()?;
                        directory.unlink(NEXT_SEGMENT_PENDING_FILE)?;
                        directory.sync()?;
                    } else {
                        if read_next_segment(directory, NEXT_SEGMENT_FILE)? != next
                            || read_next_segment(directory, NEXT_SEGMENT_PENDING_FILE)? != pending
                        {
                            return Err(io::Error::new(
                                io::ErrorKind::InvalidData,
                                "journal reservation publication changed unexpectedly",
                            ));
                        }
                        if pending > next {
                            replace_with_pending(directory)?;
                            ensure_reservation_marker(directory)?;
                            return validate_ahead(pending, segments);
                        }
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "journal pending reservation does not advance",
                        ));
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) if error.kind() == io::ErrorKind::InvalidData => {
                    directory.unlink(NEXT_SEGMENT_PENDING_FILE)?;
                    directory.sync()?;
                }
                Err(error) => return Err(error),
            }
            read_next_segment(directory, NEXT_SEGMENT_FILE)?;
            ensure_reservation_marker(directory)?;
            if segments
                .keys()
                .next_back()
                .is_some_and(|last| *last >= next)
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "journal segment reservation is not ahead of stored segments",
                ));
            }
            validate_ahead(next, segments)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let established = reservation_marker_exists(directory)?;
            let next = match read_next_segment(directory, NEXT_SEGMENT_PENDING_FILE) {
                Ok(next) => {
                    sync_pending(directory)?;
                    next
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    if established {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "established journal reservation metadata is missing",
                        ));
                    }
                    let next = next_after_segments(segments)?;
                    write_next_segment_pending(directory, next)?;
                    next
                }
                Err(error) if error.kind() == io::ErrorKind::InvalidData => {
                    directory.unlink(NEXT_SEGMENT_PENDING_FILE)?;
                    directory.sync()?;
                    let next = next_after_segments(segments)?;
                    write_next_segment_pending(directory, next)?;
                    next
                }
                Err(error) => return Err(error),
            };
            if segments
                .keys()
                .next_back()
                .is_some_and(|last| *last >= next)
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "journal pending reservation is not ahead of stored segments",
                ));
            }
            publish_pending(directory)?;
            ensure_reservation_marker(directory)?;
            validate_ahead(next, segments)
        }
        Err(error) => Err(error),
    }
}

fn ensure_reservation_marker(directory: &AnchoredDirectory) -> io::Result<()> {
    match reservation_marker_exists(directory) {
        Ok(true) => Ok(()),
        Ok(false) => create_reservation_marker(directory),
        Err(error) if error.kind() == io::ErrorKind::InvalidData => {
            // A final reservation proves initialization reached publication, so
            // an unpublished partial marker is safe to replace.
            directory.unlink(SEGMENT_RESERVATION_MARKER_FILE)?;
            directory.sync()?;
            create_reservation_marker(directory)
        }
        Err(error) => Err(error),
    }
}

fn reservation_marker_exists(directory: &AnchoredDirectory) -> io::Result<bool> {
    match directory.open_file(
        SEGMENT_RESERVATION_MARKER_FILE,
        rustix::fs::OFlags::RDONLY,
        rustix::fs::Mode::empty(),
    ) {
        Ok(file) => {
            validate_single_link(&file)?;
            let mut text = String::new();
            (&file).take(3).read_to_string(&mut text)?;
            if text != "1\n" {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "journal reservation marker is malformed",
                ));
            }
            // A previous sync may have failed after writing valid-looking
            // bytes. Re-establish both inode and directory-entry durability.
            file.sync_all()?;
            directory.sync()?;
            Ok(true)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

fn create_reservation_marker(directory: &AnchoredDirectory) -> io::Result<()> {
    let mut file = directory.open_file(
        SEGMENT_RESERVATION_MARKER_FILE,
        rustix::fs::OFlags::CREATE | rustix::fs::OFlags::EXCL | rustix::fs::OFlags::WRONLY,
        rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
    )?;
    validate_single_link(&file)?;
    file.write_all(b"1\n")?;
    file.sync_all()?;
    directory.sync()
}

fn next_after_segments(segments: &BTreeMap<u64, String>) -> io::Result<u64> {
    segments.keys().next_back().map_or(Ok(0), |last| {
        last.checked_add(1).ok_or_else(segment_exhausted)
    })
}

/// Durably advance an established reservation before exposing its segment.
///
/// The caller serializes writers and creates only the returned old value.
pub(super) fn reserve_next_segment(directory: &AnchoredDirectory, current: u64) -> io::Result<u64> {
    let stored = read_next_segment(directory, NEXT_SEGMENT_FILE)?;
    if stored != current {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "journal segment reservation changed unexpectedly",
        ));
    }
    let next = current.checked_add(1).ok_or_else(segment_exhausted)?;
    remove_unpublished_next_segment(directory)?;
    write_next_segment_pending(directory, next)?;
    // Commit the pending name before removing the only published reservation.
    directory.sync()?;
    directory.unlink(NEXT_SEGMENT_FILE)?;
    directory.sync()?;
    publish_pending(directory)?;
    Ok(next)
}

fn validate_ahead(next: u64, segments: &BTreeMap<u64, String>) -> io::Result<u64> {
    if segments
        .keys()
        .next_back()
        .is_some_and(|last| *last >= next)
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "journal segment reservation is not ahead of stored segments",
        ));
    }
    Ok(next)
}

fn sync_pending(directory: &AnchoredDirectory) -> io::Result<()> {
    directory
        .open_file(
            NEXT_SEGMENT_PENDING_FILE,
            rustix::fs::OFlags::RDONLY,
            rustix::fs::Mode::empty(),
        )?
        .sync_all()
}

fn publish_pending(directory: &AnchoredDirectory) -> io::Result<()> {
    directory.link(NEXT_SEGMENT_PENDING_FILE, NEXT_SEGMENT_FILE)?;
    directory.sync()?;
    directory.unlink(NEXT_SEGMENT_PENDING_FILE)?;
    directory.sync()
}

fn replace_with_pending(directory: &AnchoredDirectory) -> io::Result<()> {
    // Pending must survive a crash after the old final is removed.
    directory.sync()?;
    directory.unlink(NEXT_SEGMENT_FILE)?;
    directory.sync()?;
    publish_pending(directory)
}

fn same_entry(directory: &AnchoredDirectory, left: &str, right: &str) -> io::Result<bool> {
    use std::os::unix::fs::MetadataExt as _;

    let left = directory
        .open_file(left, rustix::fs::OFlags::RDONLY, rustix::fs::Mode::empty())?
        .metadata()?;
    let right = directory
        .open_file(right, rustix::fs::OFlags::RDONLY, rustix::fs::Mode::empty())?
        .metadata()?;
    Ok(left.dev() == right.dev() && left.ino() == right.ino() && left.nlink() == 2)
}

fn remove_unpublished_next_segment(directory: &AnchoredDirectory) -> io::Result<()> {
    match directory.unlink(NEXT_SEGMENT_PENDING_FILE) {
        Ok(()) => directory.sync(),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn write_next_segment_pending(directory: &AnchoredDirectory, next: u64) -> io::Result<()> {
    let mut file = directory.open_file(
        NEXT_SEGMENT_PENDING_FILE,
        rustix::fs::OFlags::CREATE | rustix::fs::OFlags::EXCL | rustix::fs::OFlags::WRONLY,
        rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
    )?;
    validate_single_link(&file)?;
    writeln!(file, "{next}")?;
    file.sync_all()
}

fn read_next_segment(directory: &AnchoredDirectory, name: &str) -> io::Result<u64> {
    let (file, value) = read_next_segment_file(directory, name)?;
    validate_single_link(&file)?;
    Ok(value)
}

fn read_next_segment_allow_publication_link(
    directory: &AnchoredDirectory,
    name: &str,
) -> io::Result<u64> {
    read_next_segment_file(directory, name).map(|(_, value)| value)
}

fn read_next_segment_file(
    directory: &AnchoredDirectory,
    name: &str,
) -> io::Result<(std::fs::File, u64)> {
    let file = directory.open_file(name, rustix::fs::OFlags::RDONLY, rustix::fs::Mode::empty())?;
    let mut text = String::new();
    (&file).take(22).read_to_string(&mut text)?;
    let value = text.strip_suffix('\n').ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "journal segment reservation is malformed",
        )
    })?;
    if value.is_empty() || (value.len() > 1 && value.starts_with('0')) || text.len() > 21 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "journal segment reservation is malformed",
        ));
    }
    let value = value.parse().map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "journal segment reservation is malformed",
        )
    })?;
    Ok((file, value))
}

fn segment_exhausted() -> io::Error {
    io::Error::other("journal segment number space is exhausted")
}

#[cfg(test)]
#[path = "segment_reservation/tests.rs"]
mod tests;
