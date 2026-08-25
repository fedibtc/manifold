use std::fs::File;
use std::io;
use std::path::{Component, Path};

/// One validated directory descriptor anchoring all journal operations.
pub(crate) struct AnchoredDirectory(pub(crate) File);

impl AnchoredDirectory {
    /// Durably create and safely open a journal directory without following links.
    pub(crate) fn open(path: &Path) -> io::Result<Self> {
        let absolute = std::path::absolute(path)?;
        let root = rustix::fs::open(
            "/",
            rustix::fs::OFlags::RDONLY
                | rustix::fs::OFlags::DIRECTORY
                | rustix::fs::OFlags::NOFOLLOW
                | rustix::fs::OFlags::CLOEXEC,
            rustix::fs::Mode::empty(),
        )?;
        let mut current = File::from(root);
        for component in absolute.components() {
            let Component::Normal(name) = component else {
                if matches!(component, Component::RootDir) {
                    continue;
                }
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "journal path contains a non-normal component",
                ));
            };
            let flags = rustix::fs::OFlags::RDONLY
                | rustix::fs::OFlags::DIRECTORY
                | rustix::fs::OFlags::NOFOLLOW
                | rustix::fs::OFlags::CLOEXEC;
            let next = match rustix::fs::openat(&current, name, flags, rustix::fs::Mode::empty()) {
                Ok(next) => next,
                Err(error) if error == rustix::io::Errno::NOENT => {
                    rustix::fs::mkdirat(
                        &current,
                        name,
                        rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR | rustix::fs::Mode::XUSR,
                    )?;
                    current.sync_all()?;
                    rustix::fs::openat(&current, name, flags, rustix::fs::Mode::empty())?
                }
                Err(error) => return Err(error.into()),
            };
            current = File::from(next);
        }
        let directory = Self(current);
        rustix::fs::fchmod(
            &directory.0,
            rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR | rustix::fs::Mode::XUSR,
        )?;
        Ok(directory)
    }

    /// Open one regular child without following a final symbolic link.
    pub(crate) fn open_file(
        &self,
        name: &str,
        flags: rustix::fs::OFlags,
        mode: rustix::fs::Mode,
    ) -> io::Result<File> {
        let fd = rustix::fs::openat(
            &self.0,
            name,
            flags
                | rustix::fs::OFlags::NOFOLLOW
                | rustix::fs::OFlags::CLOEXEC
                | rustix::fs::OFlags::NONBLOCK,
            mode,
        )?;
        let file = File::from(fd);
        if !file.metadata()?.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "journal entry is not a regular file",
            ));
        }
        Ok(file)
    }

    /// Unlink one child of the anchored directory.
    pub(crate) fn unlink(&self, name: &str) -> io::Result<()> {
        rustix::fs::unlinkat(&self.0, name, rustix::fs::AtFlags::empty()).map_err(Into::into)
    }

    /// Durably commit directory-entry changes.
    pub(crate) fn sync(&self) -> io::Result<()> {
        self.0.sync_all()
    }

    /// Publish a hard link without replacing an existing destination.
    pub(crate) fn link(&self, old: &str, new: &str) -> io::Result<()> {
        rustix::fs::linkat(&self.0, old, &self.0, new, rustix::fs::AtFlags::empty())
            .map_err(Into::into)
    }
}

/// Reject a multiply linked regular journal entry.
pub(crate) fn validate_single_link(file: &File) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        if file.metadata()?.nlink() != 1 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "journal entry has an unexpected link count",
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "anchored_directory/tests.rs"]
mod tests;
