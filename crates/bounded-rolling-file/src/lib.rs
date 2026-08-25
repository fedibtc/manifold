//! Strictly bounded rolling storage for newline-delimited records.
//!
//! Unlike ordinary logging appenders, rollover failure is returned to the
//! caller rather than ignored: continued writes must never exceed the bound.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{self, BufRead as _, Read as _, Seek as _, Write as _};
use std::path::{Path, PathBuf};

const SEGMENT_PREFIX: &str = "events-";
const SEGMENT_SUFFIX: &str = ".jsonl";
const INCARNATION_FILE: &str = "incarnation";
const INCARNATION_PENDING_FILE: &str = "incarnation.pending";
const INCARNATION_LOCK_FILE: &str = "incarnation.lock";
const INCARNATION_TEXT_BYTES: usize = 37;
const NEXT_SEGMENT_FILE: &str = "next-segment";
const NEXT_SEGMENT_PENDING_FILE: &str = "next-segment.pending";
const SEGMENT_RESERVATION_MARKER_FILE: &str = "segment-reservation-v1";

mod anchored_directory;
mod journal_incarnation;
mod segment_reservation;

use anchored_directory::{AnchoredDirectory, validate_single_link};
pub use journal_incarnation::JournalIncarnation;
use journal_incarnation::{open_incarnation_locked, read_incarnation};
use segment_reservation::{open_next_segment, reserve_next_segment};

/// Retention limits for one rolling journal.
#[derive(Clone, Copy, Debug)]
pub struct Config {
    /// Maximum size of one segment.
    pub max_file_bytes: u64,
    /// Maximum number of segments, including the active segment.
    pub max_files: usize,
}

/// A single-writer, size-bounded journal of newline-delimited records.
pub struct RollingFileAppender {
    /// Held for this writer's lifetime; prevents cross-process rotation races.
    _directory_lock: File,
    directory: AnchoredDirectory,
    config: Config,
    segments: BTreeMap<u64, String>,
    active: Option<ActiveFile>,
    next_segment: u64,
    poisoned: bool,
}

struct ActiveFile {
    file: File,
    bytes: u64,
}

/// Byte position immediately after a complete record in one segment.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReadCursor {
    pub segment: u64,
    pub offset: u64,
}

/// One bounded incremental read from a rolling journal.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReadBatch {
    /// Complete newline-terminated records, byte-for-byte as stored.
    pub records: Vec<u8>,
    /// Position after the last returned record.
    pub next_cursor: Option<ReadCursor>,
    /// The supplied cursor was no longer a valid retained position.
    pub continuity_gap: bool,
}

/// Result of an incarnation-bound journal read.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum IncarnationReadBatch {
    /// The expected incarnation is current and its records were read.
    Current {
        /// Current durable journal identity.
        incarnation: JournalIncarnation,
        /// Bounded records and next position.
        batch: ReadBatch,
    },
    /// The request or supplied cursor belongs to another storage generation.
    IncarnationChanged {
        /// Current durable journal identity.
        incarnation: JournalIncarnation,
    },
}

/// Open or create the journal's durable storage-generation identity.
///
/// Existing segment files are not modified when a legacy directory receives
/// its first identity. A malformed published identity fails closed.
pub fn open_incarnation(directory: impl AsRef<Path>) -> io::Result<JournalIncarnation> {
    let directory = AnchoredDirectory::open(directory.as_ref())?;
    let _lock = open_blocking_lock(&directory, INCARNATION_LOCK_FILE)?;
    open_incarnation_locked(&directory)
}

/// Read records only when the expected identity is the current journal identity.
///
/// Identity comparison happens while holding the incarnation lock and before
/// segment discovery or opening any segment file.
pub fn read_batch_for_incarnation(
    directory: impl AsRef<Path>,
    expected_incarnation: &str,
    cursor: Option<ReadCursor>,
    max_batch_bytes: usize,
    max_record_bytes: usize,
) -> io::Result<IncarnationReadBatch> {
    let directory = AnchoredDirectory::open(directory.as_ref())?;
    let _lock = open_blocking_lock(&directory, INCARNATION_LOCK_FILE)?;
    let incarnation = open_incarnation_locked(&directory)?;
    if expected_incarnation != incarnation.as_str() {
        return Ok(IncarnationReadBatch::IncarnationChanged { incarnation });
    }
    let batch = read_batch_anchored(&directory, cursor, max_batch_bytes, max_record_bytes)?;
    let after_read = read_incarnation(&directory, INCARNATION_FILE)?;
    if after_read != incarnation {
        return Ok(IncarnationReadBatch::IncarnationChanged {
            incarnation: after_read,
        });
    }
    Ok(IncarnationReadBatch::Current { incarnation, batch })
}

/// Read complete records for storage-internal tests without parsing JSON.
///
/// Protocol callers must use [`read_batch_for_incarnation`] instead.
#[cfg(test)]
fn read_batch(
    directory: impl AsRef<Path>,
    cursor: Option<ReadCursor>,
    max_batch_bytes: usize,
    max_record_bytes: usize,
) -> io::Result<ReadBatch> {
    let directory = AnchoredDirectory::open(directory.as_ref())?;
    read_batch_anchored(&directory, cursor, max_batch_bytes, max_record_bytes)
}

fn read_batch_anchored(
    directory: &AnchoredDirectory,
    cursor: Option<ReadCursor>,
    max_batch_bytes: usize,
    max_record_bytes: usize,
) -> io::Result<ReadBatch> {
    if max_batch_bytes == 0 || max_record_bytes == 0 || max_record_bytes > max_batch_bytes {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "journal read limits must be nonzero and fit one record",
        ));
    }

    let segments = discover_segments(directory)?;
    let Some((&oldest_segment, oldest_path)) = segments.first_key_value() else {
        return Ok(ReadBatch {
            records: Vec::new(),
            next_cursor: None,
            continuity_gap: cursor.is_some(),
        });
    };

    let mut continuity_gap = false;
    let mut opened_start = None;
    let (start_segment, start_offset) = if let Some(cursor) = cursor {
        if let Some(path) = segments.get(&cursor.segment) {
            let mut file = open_segment(directory, path, false)?;
            if cursor_is_valid(&mut file, cursor)? {
                opened_start = Some(file);
                (cursor.segment, cursor.offset)
            } else {
                continuity_gap = true;
                (oldest_segment, 0)
            }
        } else {
            continuity_gap = true;
            (oldest_segment, 0)
        }
    } else {
        (oldest_segment, 0)
    };

    if opened_start.is_none() {
        opened_start = Some(open_segment(directory, oldest_path, false)?);
    }
    let mut records = Vec::new();
    let mut next_cursor = if continuity_gap { None } else { cursor };

    'segments: for (&segment, path) in segments.range(start_segment..) {
        let offset = if segment == start_segment {
            start_offset
        } else {
            0
        };
        let file = if segment == start_segment {
            opened_start.take().expect("start segment was opened")
        } else {
            open_segment(directory, path, false)?
        };
        let mut file = std::io::BufReader::new(file);
        file.seek(io::SeekFrom::Start(offset))?;
        let mut segment_offset = offset;
        loop {
            let mut record = Vec::new();
            let read = std::io::Read::by_ref(&mut file)
                .take((max_record_bytes as u64).saturating_add(1))
                .read_until(b'\n', &mut record)?;
            if read == 0 {
                break;
            }
            if record.len() > max_record_bytes {
                segment_offset = segment_offset.saturating_add(read as u64);
                if !record.ends_with(b"\n") {
                    discard_through_newline(&mut file, &mut segment_offset)?;
                }
                next_cursor = Some(ReadCursor {
                    segment,
                    offset: segment_offset,
                });
                continuity_gap = true;
                break 'segments;
            }
            if !record.ends_with(b"\n") {
                break 'segments;
            }
            if records.len().saturating_add(record.len()) > max_batch_bytes {
                break 'segments;
            }
            records.extend_from_slice(&record);
            segment_offset = segment_offset.saturating_add(record.len() as u64);
            next_cursor = Some(ReadCursor {
                segment,
                offset: segment_offset,
            });
        }
    }

    Ok(ReadBatch {
        records,
        next_cursor,
        continuity_gap,
    })
}

/// Discard an oversized record without allocating in proportion to corrupt
/// input. The cursor advances to its newline, or to the current EOF when an
/// incomplete tail is already too large to ever return.
fn discard_through_newline(reader: &mut impl io::BufRead, offset: &mut u64) -> io::Result<()> {
    loop {
        let buffer = reader.fill_buf()?;
        if buffer.is_empty() {
            return Ok(());
        }
        let consumed = buffer
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(buffer.len(), |index| index + 1);
        let found_newline = buffer.get(consumed - 1) == Some(&b'\n');
        reader.consume(consumed);
        *offset = offset.saturating_add(consumed as u64);
        if found_newline {
            return Ok(());
        }
    }
}

fn cursor_is_valid(file: &mut File, cursor: ReadCursor) -> io::Result<bool> {
    if cursor.offset > file.metadata()?.len() {
        return Ok(false);
    }
    if cursor.offset == 0 {
        return Ok(true);
    }
    file.seek(io::SeekFrom::Start(cursor.offset - 1))?;
    let mut previous = [0];
    file.read_exact(&mut previous)?;
    Ok(previous == *b"\n")
}

impl RollingFileAppender {
    /// Open or create a journal, repair and seal old segments, and prepare a
    /// fresh durably reserved coordinate for the next append.
    pub fn open(directory: impl Into<PathBuf>, config: Config) -> io::Result<Self> {
        validate_config(config)?;
        let directory = AnchoredDirectory::open(&directory.into())?;
        let directory_lock = open_directory_lock(&directory)?;
        let _incarnation_lock = open_blocking_lock(&directory, INCARNATION_LOCK_FILE)?;
        open_incarnation_locked(&directory)?;
        let mut segments = discover_segments(&directory)?;
        let next_segment = open_next_segment(&directory, &segments)?;
        remove_oversized(&directory, &mut segments, config.max_file_bytes)?;
        while segments.len() > config.max_files {
            remove_oldest(&directory, &mut segments)?;
        }
        if let Some(name) = segments.last_key_value().map(|(_, name)| name) {
            repair_tail(&directory, name, config.max_file_bytes)?;
        }
        for name in segments.values() {
            seal_segment(&directory, name)?;
        }
        directory.sync()?;

        Ok(Self {
            _directory_lock: directory_lock,
            directory,
            config,
            segments,
            // Never append after reopening. A newly reserved segment prevents
            // crash rollback or repair from making an old coordinate valid for
            // unrelated bytes.
            active: None,
            next_segment,
            poisoned: false,
        })
    }

    /// Append one complete newline-terminated record.
    ///
    /// A write error permanently poisons this appender because rollback cannot
    /// be confirmed. A successful return means the complete record has been
    /// synchronously persisted. Later appends fail while the writer lock remains
    /// held.
    pub fn append_record(&mut self, record: &[u8]) -> io::Result<()> {
        if self.poisoned {
            return Err(io::Error::other("rolling file appender is poisoned"));
        }
        if record.is_empty() || !record.ends_with(b"\n") {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "rolling record must be nonempty and newline-terminated",
            ));
        }
        if record.len() as u64 > self.config.max_file_bytes {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "rolling record exceeds maximum file size",
            ));
        }

        let needs_rotation = self.active.as_ref().is_none_or(|active| {
            active.bytes > 0
                && active.bytes.saturating_add(record.len() as u64) > self.config.max_file_bytes
        });
        if needs_rotation {
            self.rotate()?;
        }

        let active = self
            .active
            .as_mut()
            .expect("rotation always creates an active file");
        if let Err(error) = active.file.write_all(record) {
            let _ = active.file.set_len(active.bytes);
            self.poisoned = true;
            return Err(error);
        }
        if let Err(error) = active.file.sync_data() {
            self.poisoned = true;
            return Err(error);
        }
        active.bytes = active.bytes.saturating_add(record.len() as u64);
        Ok(())
    }

    fn rotate(&mut self) -> io::Result<()> {
        self.active = None;
        while self.segments.len() >= self.config.max_files {
            remove_oldest(&self.directory, &mut self.segments)?;
        }
        let number = self.next_segment;
        self.next_segment = reserve_next_segment(&self.directory, number)?;
        let name = segment_name(number);
        let file = open_segment(&self.directory, &name, true)?;
        self.directory.sync()?;
        self.segments.insert(number, name);
        self.active = Some(ActiveFile { file, bytes: 0 });
        Ok(())
    }
}

fn validate_config(config: Config) -> io::Result<()> {
    if config.max_file_bytes == 0 || config.max_files == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "rolling file limits must be nonzero",
        ));
    }
    Ok(())
}

fn open_directory_lock(directory: &AnchoredDirectory) -> io::Result<File> {
    open_lock(directory, "writer.lock", false)
}

fn open_lock(directory: &AnchoredDirectory, name: &str, blocking: bool) -> io::Result<File> {
    let file = directory.open_file(
        name,
        rustix::fs::OFlags::CREATE | rustix::fs::OFlags::RDWR,
        rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
    )?;
    validate_single_link(&file)?;
    if blocking {
        file.lock()?;
    } else {
        file.try_lock()?;
    }
    Ok(file)
}

fn open_blocking_lock(directory: &AnchoredDirectory, name: &str) -> io::Result<File> {
    open_lock(directory, name, true)
}

fn segment_name(number: u64) -> String {
    format!("{SEGMENT_PREFIX}{number}{SEGMENT_SUFFIX}")
}

fn parse_segment_name(name: &str) -> Option<u64> {
    let number = name
        .strip_prefix(SEGMENT_PREFIX)?
        .strip_suffix(SEGMENT_SUFFIX)?;
    if number.is_empty() || (number.len() > 1 && number.starts_with('0')) {
        return None;
    }
    number.parse().ok()
}

fn discover_segments(directory: &AnchoredDirectory) -> io::Result<BTreeMap<u64, String>> {
    let mut segments = BTreeMap::new();
    let mut entries = rustix::fs::Dir::read_from(&directory.0)?;
    for entry in &mut entries {
        let entry = entry?;
        let Ok(name) = entry.file_name().to_str() else {
            continue;
        };
        if let Some(number) = parse_segment_name(name) {
            if !entry.file_type().is_file() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "journal segment is not a regular file",
                ));
            }
            segments.insert(number, name.to_owned());
        }
    }
    Ok(segments)
}

fn remove_oldest(
    directory: &AnchoredDirectory,
    segments: &mut BTreeMap<u64, String>,
) -> io::Result<()> {
    let Some((&number, name)) = segments.first_key_value() else {
        return Ok(());
    };
    open_segment(directory, name, false)?;
    directory.unlink(name)?;
    directory.sync()?;
    segments.remove(&number);
    Ok(())
}

fn remove_oversized(
    directory: &AnchoredDirectory,
    segments: &mut BTreeMap<u64, String>,
    max_file_bytes: u64,
) -> io::Result<()> {
    let mut oversized = Vec::new();
    for (&number, name) in segments.iter() {
        if open_segment(directory, name, false)?.metadata()?.len() > max_file_bytes {
            oversized.push(number);
        }
    }
    for number in oversized {
        let name = segments
            .get(&number)
            .expect("oversized segment came from this map");
        directory.unlink(name)?;
        directory.sync()?;
        segments.remove(&number);
    }
    Ok(())
}

fn open_segment(directory: &AnchoredDirectory, name: &str, create: bool) -> io::Result<File> {
    let flags = if create {
        rustix::fs::OFlags::CREATE
            | rustix::fs::OFlags::EXCL
            | rustix::fs::OFlags::APPEND
            | rustix::fs::OFlags::WRONLY
    } else {
        rustix::fs::OFlags::RDONLY
    };
    let file = directory.open_file(name, flags, rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR)?;
    validate_single_link(&file)?;
    Ok(file)
}

fn seal_segment(directory: &AnchoredDirectory, name: &str) -> io::Result<()> {
    let file = open_segment(directory, name, false)?;
    rustix::fs::fchmod(&file, rustix::fs::Mode::RUSR)?;
    file.sync_all()
}

fn repair_tail(directory: &AnchoredDirectory, name: &str, max_file_bytes: u64) -> io::Result<()> {
    let mut file =
        directory.open_file(name, rustix::fs::OFlags::RDONLY, rustix::fs::Mode::empty())?;
    validate_single_link(&file)?;
    let mut bytes = Vec::new();
    std::io::Read::take(&mut file, max_file_bytes.saturating_add(1)).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > max_file_bytes {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "rolling segment grew beyond its configured limit during repair",
        ));
    }
    let valid_len = bytes
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(0, |index| index + 1);
    if valid_len != bytes.len() {
        drop(file);
        let file =
            directory.open_file(name, rustix::fs::OFlags::RDWR, rustix::fs::Mode::empty())?;
        validate_single_link(&file)?;
        file.set_len(valid_len as u64)?;
        file.sync_data()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt as _;

    use super::*;

    fn config() -> Config {
        Config {
            max_file_bytes: 8,
            max_files: 2,
        }
    }

    fn test_segments(directory: &Path) -> BTreeMap<u64, PathBuf> {
        discover_segments(&AnchoredDirectory::open(directory).unwrap())
            .unwrap()
            .into_iter()
            .map(|(number, name)| (number, directory.join(name)))
            .collect()
    }

    #[test]
    fn rotates_prunes_and_resumes() {
        let directory = tempfile::tempdir().unwrap();
        {
            let mut appender = RollingFileAppender::open(directory.path(), config()).unwrap();
            appender.append_record(b"1111\n").unwrap();
            appender.append_record(b"2222\n").unwrap();
            appender.append_record(b"3333\n").unwrap();
        }
        {
            let mut appender = RollingFileAppender::open(directory.path(), config()).unwrap();
            appender.append_record(b"4444\n").unwrap();
        }

        let segments = test_segments(directory.path());
        assert_eq!(segments.len(), 2);
        let contents = segments
            .into_values()
            .map(std::fs::read_to_string)
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(contents, ["3333\n", "4444\n"]);
    }

    #[test]
    fn names_segments_without_padding() {
        let directory = tempfile::tempdir().unwrap();
        let mut appender = RollingFileAppender::open(directory.path(), config()).unwrap();
        appender.append_record(b"new\n").unwrap();
        drop(appender);

        assert!(directory.path().join("events-0.jsonl").exists());
        assert_eq!(segment_name(42), "events-42.jsonl");
        assert_eq!(parse_segment_name("events-42.jsonl"), Some(42));
        assert_eq!(parse_segment_name("events-00042.jsonl"), None);
    }

    #[test]
    fn reads_complete_records_incrementally_across_segments() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join(segment_name(4)), b"one\ntwo\n").unwrap();
        std::fs::write(directory.path().join(segment_name(5)), b"three\npartial").unwrap();

        let first = read_batch(directory.path(), None, 8, 8).unwrap();
        assert_eq!(first.records, b"one\ntwo\n");
        assert_eq!(
            first.next_cursor,
            Some(ReadCursor {
                segment: 4,
                offset: 8
            })
        );
        assert!(!first.continuity_gap);

        let second = read_batch(directory.path(), first.next_cursor, 8, 8).unwrap();
        assert_eq!(second.records, b"three\n");
        assert_eq!(
            second.next_cursor,
            Some(ReadCursor {
                segment: 5,
                offset: 6
            })
        );
        assert!(!second.continuity_gap);

        std::fs::write(directory.path().join(segment_name(5)), b"three\npartial\n").unwrap();
        let third = read_batch(directory.path(), second.next_cursor, 8, 8).unwrap();
        assert_eq!(third.records, b"partial\n");
    }

    #[test]
    fn missing_or_invalid_cursor_restarts_at_oldest_with_a_gap() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join(segment_name(7)), b"oldest\n").unwrap();

        for cursor in [
            ReadCursor {
                segment: 6,
                offset: 4,
            },
            ReadCursor {
                segment: 7,
                offset: 3,
            },
        ] {
            let batch = read_batch(directory.path(), Some(cursor), 16, 16).unwrap();
            assert_eq!(batch.records, b"oldest\n");
            assert!(batch.continuity_gap);
            assert_eq!(
                batch.next_cursor,
                Some(ReadCursor {
                    segment: 7,
                    offset: 7
                })
            );
        }
    }

    #[test]
    fn oversized_record_is_skipped_with_a_gap_instead_of_wedging_the_reader() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join(segment_name(7)), b"ok\n12345\nnext\n").unwrap();

        let first = read_batch(directory.path(), None, 16, 5).unwrap();
        assert_eq!(first.records, b"ok\n");
        assert_eq!(
            first.next_cursor,
            Some(ReadCursor {
                segment: 7,
                offset: 9,
            })
        );
        assert!(first.continuity_gap);

        let second = read_batch(directory.path(), first.next_cursor, 16, 5).unwrap();
        assert_eq!(second.records, b"next\n");
        assert!(!second.continuity_gap);
    }

    #[test]
    fn repairs_an_incomplete_tail() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join(segment_name(0)), b"complete\npartial").unwrap();

        let mut appender = RollingFileAppender::open(
            directory.path(),
            Config {
                max_file_bytes: 64,
                max_files: 2,
            },
        )
        .unwrap();
        appender.append_record(b"next\n").unwrap();

        assert_eq!(
            std::fs::read(directory.path().join(segment_name(0))).unwrap(),
            b"complete\n"
        );
        assert_eq!(
            std::fs::read(directory.path().join(segment_name(1))).unwrap(),
            b"next\n"
        );
    }

    #[test]
    fn removes_a_preexisting_oversized_segment_before_repair() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join(segment_name(0)), b"oversized\n").unwrap();

        let mut appender = RollingFileAppender::open(directory.path(), config()).unwrap();
        assert!(appender.segments.is_empty());
        appender.append_record(b"new\n").unwrap();
        drop(appender);

        let contents = test_segments(directory.path())
            .into_values()
            .map(std::fs::read_to_string)
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(contents, ["new\n"]);
    }

    #[test]
    fn lowering_the_limit_removes_an_old_valid_segment() {
        let directory = tempfile::tempdir().unwrap();
        {
            let mut appender = RollingFileAppender::open(
                directory.path(),
                Config {
                    max_file_bytes: 64,
                    max_files: 2,
                },
            )
            .unwrap();
            appender.append_record(b"previously-valid\n").unwrap();
        }

        let mut appender = RollingFileAppender::open(directory.path(), config()).unwrap();
        assert!(appender.segments.is_empty());
        appender.append_record(b"new\n").unwrap();
    }

    #[test]
    fn rejects_invalid_or_oversized_records() {
        let directory = tempfile::tempdir().unwrap();
        let mut appender = RollingFileAppender::open(directory.path(), config()).unwrap();

        assert_eq!(
            appender
                .append_record(b"missing newline")
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidInput
        );
        assert_eq!(
            appender.append_record(b"too-long\n").unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        assert!(test_segments(directory.path()).is_empty());
    }

    #[test]
    fn refuses_a_second_writer() {
        let directory = tempfile::tempdir().unwrap();
        let first = RollingFileAppender::open(directory.path(), config()).unwrap();
        let Err(error) = RollingFileAppender::open(directory.path(), config()) else {
            panic!("the first appender must hold the writer lock");
        };
        assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
        drop(first);
        RollingFileAppender::open(directory.path(), config()).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn restricts_permissions() {
        let directory = tempfile::tempdir().unwrap();
        let journal = directory.path().join("journal");
        let mut appender = RollingFileAppender::open(&journal, config()).unwrap();
        appender.append_record(b"record\n").unwrap();

        assert_eq!(
            std::fs::metadata(&journal).unwrap().permissions().mode() & 0o777,
            0o700
        );
        let segment = test_segments(&journal).into_values().next().unwrap();
        assert_eq!(
            std::fs::metadata(segment).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            std::fs::metadata(journal.join(INCARNATION_FILE))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }

    #[test]
    fn write_failure_permanently_poisons_the_appender() {
        let directory = tempfile::tempdir().unwrap();
        let mut appender = RollingFileAppender::open(directory.path(), config()).unwrap();
        appender.append_record(b"one\n").unwrap();
        let path = appender.segments.last_key_value().unwrap().1.clone();

        appender.active.as_mut().unwrap().file = appender
            .directory
            .open_file(&path, rustix::fs::OFlags::RDONLY, rustix::fs::Mode::empty())
            .unwrap();
        assert!(appender.append_record(b"two\n").is_err());

        appender.active.as_mut().unwrap().file = appender
            .directory
            .open_file(
                &path,
                rustix::fs::OFlags::APPEND | rustix::fs::OFlags::WRONLY,
                rustix::fs::Mode::empty(),
            )
            .unwrap();
        assert_eq!(
            appender.append_record(b"two\n").unwrap_err().kind(),
            io::ErrorKind::Other
        );
    }

    #[test]
    fn incarnation_survives_empty_open_restart_and_rotation() {
        let directory = tempfile::tempdir().unwrap();
        let first = open_incarnation(directory.path()).unwrap();
        {
            let mut appender = RollingFileAppender::open(directory.path(), config()).unwrap();
            appender.append_record(b"1111\n").unwrap();
            appender.append_record(b"2222\n").unwrap();
            appender.append_record(b"3333\n").unwrap();
        }
        let reopened = open_incarnation(directory.path()).unwrap();

        assert_eq!(first, reopened);
        assert_eq!(
            std::fs::read_to_string(directory.path().join(INCARNATION_FILE)).unwrap(),
            format!("{}\n", first.as_str())
        );
        assert!(!directory.path().join(INCARNATION_PENDING_FILE).exists());
        assert!(!format!("{first:?}").contains(first.as_str()));
    }

    #[test]
    fn repeated_quiet_reopen_accepts_already_sealed_complete_tail() {
        let directory = tempfile::tempdir().unwrap();
        {
            let mut writer = RollingFileAppender::open(directory.path(), config()).unwrap();
            writer.append_record(b"one\n").unwrap();
        }
        RollingFileAppender::open(directory.path(), config()).unwrap();
        RollingFileAppender::open(directory.path(), config()).unwrap();
    }

    #[test]
    fn multiply_linked_incomplete_tail_is_rejected_before_modification() {
        let directory = tempfile::tempdir().unwrap();
        let segment = directory.path().join(segment_name(0));
        let alias = directory.path().join("alias");
        std::fs::write(&segment, b"complete\npartial").unwrap();
        std::fs::hard_link(&segment, &alias).unwrap();

        assert!(RollingFileAppender::open(directory.path(), config()).is_err());
        assert_eq!(std::fs::read(&segment).unwrap(), b"complete\npartial");
        assert_eq!(std::fs::read(&alias).unwrap(), b"complete\npartial");
    }

    #[test]
    fn legacy_segments_receive_identity_without_record_loss() {
        let directory = tempfile::tempdir().unwrap();
        let segment = directory.path().join(segment_name(7));
        std::fs::write(&segment, b"preserved\n").unwrap();

        open_incarnation(directory.path()).unwrap();

        assert_eq!(std::fs::read(segment).unwrap(), b"preserved\n");
    }

    #[test]
    fn recreated_directory_gets_a_new_incarnation() {
        let parent = tempfile::tempdir().unwrap();
        let directory = parent.path().join("journal");
        let first = open_incarnation(&directory).unwrap();
        std::fs::remove_dir_all(&directory).unwrap();
        let second = open_incarnation(&directory).unwrap();

        assert_ne!(first, second);
    }

    #[test]
    fn malformed_published_identity_fails_without_regeneration() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join(INCARNATION_FILE), b"partial").unwrap();

        let first = open_incarnation(directory.path()).unwrap_err();
        let second = open_incarnation(directory.path()).unwrap_err();

        assert_eq!(first.kind(), io::ErrorKind::InvalidData);
        assert_eq!(second.kind(), io::ErrorKind::InvalidData);
        assert_eq!(
            std::fs::read(directory.path().join(INCARNATION_FILE)).unwrap(),
            b"partial"
        );
    }

    #[test]
    fn rejects_wrong_version_noncanonical_and_extra_identity_bytes() {
        for value in [
            format!("{}\n", uuid::Uuid::nil()),
            format!("{}\n", uuid::Uuid::now_v7().to_string().to_uppercase()),
            format!("{}\nextra", uuid::Uuid::now_v7()),
        ] {
            let directory = tempfile::tempdir().unwrap();
            std::fs::write(directory.path().join(INCARNATION_FILE), value).unwrap();
            assert_eq!(
                open_incarnation(directory.path()).unwrap_err().kind(),
                io::ErrorKind::InvalidData
            );
        }
    }

    #[test]
    fn interrupted_pending_write_is_repaired_but_valid_pending_is_preserved() {
        let directory = tempfile::tempdir().unwrap();
        let pending = directory.path().join(INCARNATION_PENDING_FILE);
        std::fs::write(&pending, b"partial").unwrap();
        let repaired = open_incarnation(directory.path()).unwrap();
        assert_ne!(repaired.as_str(), "partial");

        std::fs::remove_file(directory.path().join(INCARNATION_FILE)).unwrap();
        let preserved = uuid::Uuid::now_v7().to_string();
        std::fs::write(&pending, format!("{preserved}\n")).unwrap();
        let reopened = open_incarnation(directory.path()).unwrap();
        assert_eq!(reopened.as_str(), preserved);
    }

    #[cfg(unix)]
    #[test]
    fn crash_after_publication_cleans_same_inode_pending_link() {
        use std::os::unix::fs::MetadataExt as _;

        let directory = tempfile::tempdir().unwrap();
        let value = uuid::Uuid::now_v7().to_string();
        let pending = directory.path().join(INCARNATION_PENDING_FILE);
        let published = directory.path().join(INCARNATION_FILE);
        std::fs::write(&pending, format!("{value}\n")).unwrap();
        std::fs::hard_link(&pending, &published).unwrap();
        assert_eq!(
            std::fs::metadata(&pending).unwrap().ino(),
            std::fs::metadata(&published).unwrap().ino()
        );

        let incarnation = open_incarnation(directory.path()).unwrap();

        assert_eq!(incarnation.as_str(), value);
        assert!(!pending.exists());
        assert!(published.exists());
    }

    #[test]
    fn concurrent_legacy_initializers_publish_one_identity() {
        let directory = std::sync::Arc::new(tempfile::tempdir().unwrap());
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
        let threads = (0..8)
            .map(|index| {
                let directory = std::sync::Arc::clone(&directory);
                let barrier = std::sync::Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    if index == 0 {
                        let writer = RollingFileAppender::open(directory.path(), config()).unwrap();
                        let incarnation = open_incarnation(directory.path()).unwrap();
                        drop(writer);
                        incarnation
                    } else if index == 1 {
                        match read_batch_for_incarnation(
                            directory.path(),
                            "00000000-0000-7000-8000-000000000000",
                            None,
                            8,
                            8,
                        )
                        .unwrap()
                        {
                            IncarnationReadBatch::IncarnationChanged { incarnation } => incarnation,
                            IncarnationReadBatch::Current { .. } => {
                                panic!("stale fetch must report the initialized incarnation")
                            }
                        }
                    } else {
                        open_incarnation(directory.path()).unwrap()
                    }
                })
            })
            .collect::<Vec<_>>();
        let identities = threads
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .collect::<Vec<_>>();

        assert!(identities.iter().all(|identity| identity == &identities[0]));
    }

    #[test]
    fn oversized_removal_does_not_reuse_its_segment_number() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join(segment_name(41)), b"oversized\n").unwrap();
        let mut appender = RollingFileAppender::open(
            directory.path(),
            Config {
                max_file_bytes: 4,
                max_files: 1,
            },
        )
        .unwrap();
        appender.append_record(b"ok\n").unwrap();

        assert!(!directory.path().join(segment_name(41)).exists());
        assert!(directory.path().join(segment_name(42)).exists());
    }

    #[test]
    fn maximum_segment_number_fails_closed() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join(segment_name(u64::MAX)), b"record\n").unwrap();

        assert_eq!(
            RollingFileAppender::open(directory.path(), config())
                .err()
                .expect("segment namespace must be exhausted")
                .kind(),
            io::ErrorKind::Other
        );
    }

    #[test]
    fn live_directory_replacement_cannot_mix_storage_generations() {
        let parent = tempfile::tempdir().unwrap();
        let live = parent.path().join("journal");
        let displaced = parent.path().join("displaced");
        let mut writer = RollingFileAppender::open(&live, config()).unwrap();
        let old_incarnation = open_incarnation(&live).unwrap();
        std::fs::rename(&live, &displaced).unwrap();
        let new_incarnation = open_incarnation(&live).unwrap();

        writer.append_record(b"old\n").unwrap();

        assert_ne!(old_incarnation, new_incarnation);
        assert_eq!(
            std::fs::read(displaced.join(segment_name(0))).unwrap(),
            b"old\n"
        );
        assert!(!live.join(segment_name(0)).exists());
    }

    #[test]
    fn stale_incarnation_returns_before_segment_discovery() {
        let directory = tempfile::tempdir().unwrap();
        let incarnation = open_incarnation(directory.path()).unwrap();
        std::os::unix::fs::symlink(
            directory.path().join("missing"),
            directory.path().join(segment_name(0)),
        )
        .unwrap();

        let stale = read_batch_for_incarnation(
            directory.path(),
            "different",
            Some(ReadCursor {
                segment: 0,
                offset: 0,
            }),
            8,
            8,
        )
        .unwrap();
        assert!(matches!(
            stale,
            IncarnationReadBatch::IncarnationChanged { .. }
        ));
        assert!(
            read_batch_for_incarnation(directory.path(), incarnation.as_str(), None, 16, 16)
                .is_err()
        );
    }
}
