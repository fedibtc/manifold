use std::ffi::{CString, OsStr, OsString};
use std::fs::{File, OpenOptions};
use std::io::Read as _;
use std::os::fd::{AsRawFd as _, FromRawFd as _};
use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::fs::{MetadataExt as _, OpenOptionsExt as _};
use std::path::{Path, PathBuf};

use anyhow::{Context as _, ensure};

const JOURNAL_SUFFIX: &str = ".fi-cli-in-progress";
const MAX_TOKEN_BYTES: u64 = 256 * 1024;

/// A validated, restart-safe bearer-token import journal.
pub(crate) struct FundingTokenJournal {
    /// Trusted handle for journal transitions.
    directory: File,
    /// Journal entry name relative to `directory`.
    journal_name: OsString,
    /// Validated journal inode retained until completion.
    journal_file: File,
    /// Validated token contents.
    token: String,
    /// Path retained for operator-facing errors.
    path: PathBuf,
}

impl FundingTokenJournal {
    /// Moves a safe source into the journal or resumes a safe existing journal.
    pub(crate) fn prepare(source: &Path) -> anyhow::Result<Self> {
        let parent = source
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let source_name = source
            .file_name()
            .context("--funding-token-file must name a file")?;
        let mut journal_name = source_name.to_os_string();
        journal_name.push(JOURNAL_SUFFIX);
        let path = source.with_file_name(&journal_name);
        let directory = open_directory(parent)?;

        let source_file = open_entry(&directory, source_name)?;
        let journal_file = open_entry(&directory, &journal_name)?;
        let mut file = match (source_file, journal_file) {
            (Some(file), None) => {
                validate_token_file(&file, source)?;
                file.sync_all()
                    .with_context(|| format!("sync funding token {}", source.display()))?;
                publish_source(&directory, source_name, &journal_name, &file, source, &path)?;
                file
            }
            (None, Some(file)) => {
                validate_token_file(&file, &path)?;
                file.sync_all()
                    .with_context(|| format!("sync funding token journal {}", path.display()))?;
                file
            }
            (Some(_), Some(_)) => {
                anyhow::bail!(
                    "both funding token {} and restart journal {} exist; refusing an ambiguous import",
                    source.display(),
                    path.display()
                );
            }
            (None, None) => {
                anyhow::bail!(
                    "neither funding token {} nor restart journal {} exists",
                    source.display(),
                    path.display()
                );
            }
        };
        let token = read_bounded(&mut file, &path)?;
        Ok(Self {
            directory,
            journal_name,
            journal_file: file,
            token,
            path,
        })
    }

    /// Returns the validated bearer token.
    pub(crate) fn token(&self) -> &str {
        &self.token
    }

    /// Deletes the confirmed journal and durably records the transition.
    pub(crate) fn complete(self) -> anyhow::Result<()> {
        let current = open_entry(&self.directory, &self.journal_name)?.with_context(|| {
            format!("funding token journal {} disappeared", self.path.display())
        })?;
        ensure!(
            same_file(&self.journal_file, &current)?,
            "funding token journal {} was replaced",
            self.path.display()
        );
        unlink_entry(&self.directory, &self.journal_name).with_context(|| {
            format!(
                "delete confirmed funding token journal {}",
                self.path.display()
            )
        })?;
        let parent = self.path.parent().unwrap_or_else(|| Path::new("."));
        sync_directory(&self.directory, parent)
    }
}

fn publish_source(
    directory: &File,
    source_name: &OsStr,
    journal_name: &OsStr,
    source_file: &File,
    source_path: &Path,
    journal_path: &Path,
) -> anyhow::Result<()> {
    publish_source_with_hook(
        directory,
        source_name,
        journal_name,
        source_file,
        source_path,
        journal_path,
        || Ok(()),
    )
}

fn publish_source_with_hook(
    directory: &File,
    source_name: &OsStr,
    journal_name: &OsStr,
    source_file: &File,
    source_path: &Path,
    journal_path: &Path,
    after_rename: impl FnOnce() -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    rename_entry(directory, source_name, journal_name).with_context(|| {
        format!(
            "move funding token {} to restart journal {}",
            source_path.display(),
            journal_path.display()
        )
    })?;
    let hook_result = after_rename();
    let parent = journal_path.parent().unwrap_or_else(|| Path::new("."));
    sync_directory(directory, parent)?;
    hook_result?;
    let renamed = open_entry(directory, journal_name)?.with_context(|| {
        format!(
            "funding token journal {} disappeared",
            journal_path.display()
        )
    })?;
    ensure!(
        same_file(source_file, &renamed)?,
        "funding token {} was replaced while creating journal {}",
        source_path.display(),
        journal_path.display()
    );
    Ok(())
}

fn same_file(left: &File, right: &File) -> anyhow::Result<bool> {
    let left = left
        .metadata()
        .context("inspect retained funding token journal")?;
    let right = right
        .metadata()
        .context("inspect current funding token journal")?;
    Ok(left.dev() == right.dev() && left.ino() == right.ino())
}

fn open_directory(path: &Path) -> anyhow::Result<File> {
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_DIRECTORY)
        .open(path)
        .with_context(|| format!("open funding token directory {}", path.display()))
}

fn open_entry(directory: &File, name: &OsStr) -> anyhow::Result<Option<File>> {
    let name = c_string(name)?;
    // SAFETY: `name` is NUL-terminated, and the returned owned descriptor is
    // immediately wrapped in `File`.
    let fd = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK,
        )
    };
    if 0 <= fd {
        // SAFETY: `openat` returned a new owned descriptor.
        return Ok(Some(unsafe { File::from_raw_fd(fd) }));
    }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ENOENT) {
        Ok(None)
    } else {
        Err(error).context("open funding token without following symlinks")
    }
}

fn validate_token_file(file: &File, path: &Path) -> anyhow::Result<()> {
    let metadata = file
        .metadata()
        .with_context(|| format!("inspect funding token {}", path.display()))?;
    ensure!(
        metadata.file_type().is_file(),
        "funding token {} is not a regular file",
        path.display()
    );
    // SAFETY: `geteuid` has no preconditions and no failure mode.
    let effective_uid = unsafe { libc::geteuid() };
    ensure!(
        metadata.uid() == effective_uid,
        "funding token {} is not owned by the current user",
        path.display()
    );
    ensure!(
        metadata.mode() & 0o7777 == 0o600,
        "funding token {} must have mode 0600",
        path.display()
    );
    Ok(())
}

fn read_bounded(file: &mut File, path: &Path) -> anyhow::Result<String> {
    let mut bytes = Vec::new();
    file.take(MAX_TOKEN_BYTES + 1)
        .read_to_end(&mut bytes)
        .with_context(|| format!("read funding token journal {}", path.display()))?;
    ensure!(
        bytes.len() as u64 <= MAX_TOKEN_BYTES,
        "funding token journal {} exceeds {MAX_TOKEN_BYTES} bytes",
        path.display()
    );
    String::from_utf8(bytes)
        .with_context(|| format!("funding token journal {} is not UTF-8", path.display()))
}

fn rename_entry(directory: &File, source: &OsStr, destination: &OsStr) -> anyhow::Result<()> {
    let source = c_string(source)?;
    let destination = c_string(destination)?;
    #[cfg(any(target_os = "linux", target_os = "android"))]
    // SAFETY: both names are valid C strings and both directory descriptors
    // remain open for the duration of the call. `RENAME_NOREPLACE` prevents a
    // competing journal from being overwritten.
    let result = unsafe {
        libc::syscall(
            libc::SYS_renameat2,
            directory.as_raw_fd(),
            source.as_ptr(),
            directory.as_raw_fd(),
            destination.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    #[cfg(target_vendor = "apple")]
    // SAFETY: both names are valid C strings and both directory descriptors
    // remain open for the duration of the call. `RENAME_EXCL` is Darwin's
    // no-replace guarantee, equivalent to Linux `RENAME_NOREPLACE`.
    let result = i64::from(unsafe {
        libc::renameatx_np(
            directory.as_raw_fd(),
            source.as_ptr(),
            directory.as_raw_fd(),
            destination.as_ptr(),
            libc::RENAME_EXCL,
        )
    });
    #[cfg(not(any(target_os = "linux", target_os = "android", target_vendor = "apple")))]
    {
        let _ = (directory, source, destination);
        anyhow::bail!(
            "atomic no-replace funding token journal move is unsupported on this platform"
        );
    }
    #[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error()).context("rename funding token journal entry")
    }
}

fn unlink_entry(directory: &File, name: &OsStr) -> anyhow::Result<()> {
    let name = c_string(name)?;
    // SAFETY: `name` is a valid C string and the directory descriptor remains
    // open for the duration of the call.
    let result = unsafe { libc::unlinkat(directory.as_raw_fd(), name.as_ptr(), 0) };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error()).context("unlink funding token journal entry")
    }
}

fn sync_directory(directory: &File, path: &Path) -> anyhow::Result<()> {
    directory
        .sync_all()
        .with_context(|| format!("sync funding token directory {}", path.display()))
}

fn c_string(value: &OsStr) -> anyhow::Result<CString> {
    CString::new(value.as_bytes()).context("funding token filename contains a NUL byte")
}

#[cfg(test)]
pub(crate) fn journal_path(source: &Path) -> anyhow::Result<PathBuf> {
    let name = source
        .file_name()
        .context("--funding-token-file must name a file")?;
    let mut journal_name = name.to_os_string();
    journal_name.push(JOURNAL_SUFFIX);
    Ok(source.with_file_name(journal_name))
}

#[cfg(test)]
#[path = "funding_token_journal/tests.rs"]
mod tests;
