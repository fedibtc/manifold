use fs2::FileExt as _;
use std::{
    fs::{File, OpenOptions},
    io,
    os::unix::fs::{MetadataExt as _, OpenOptionsExt as _, PermissionsExt as _},
    path::Path,
};

/// Lifetime guard proving exclusive ownership of the collector data root.
pub(crate) struct DataRootLock(File);

impl DataRootLock {
    /// Acquire the data-root lock without waiting for another process.
    pub(crate) fn acquire(root: &Path) -> io::Result<Self> {
        // SAFETY: `umask` accepts every mode value and touches no memory. The
        // collector is a dedicated process, so retaining 0077 is intentional.
        unsafe {
            libc::umask(0o077);
        }
        if !root.exists() {
            let parent = root.parent().ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "data root has no parent")
            })?;
            std::fs::create_dir_all(parent)?;
            std::fs::create_dir(root)?;
            std::fs::set_permissions(root, std::fs::Permissions::from_mode(0o700))?;
        }
        verify_root(root)?;
        preflight_known_paths(root)?;
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .mode(0o600)
            .open(root.join("collector.lock"))?;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        file.try_lock_exclusive()
            .map_err(|error| io::Error::new(io::ErrorKind::WouldBlock, error))?;
        Ok(Self(file))
    }
}

fn verify_root(root: &Path) -> io::Result<()> {
    let metadata = std::fs::symlink_metadata(root)?;
    let current_uid = effective_uid();
    if !metadata.file_type().is_dir()
        || metadata.file_type().is_symlink()
        || metadata.uid() != current_uid
        || metadata.permissions().mode() & 0o777 != 0o700
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "data root must be an owned real 0700 directory",
        ));
    }
    Ok(())
}

fn preflight_known_paths(root: &Path) -> io::Result<()> {
    for name in [
        "collector.lock",
        "state.sqlite",
        "state.sqlite-wal",
        "state.sqlite-shm",
    ] {
        let path = root.join(name);
        match std::fs::symlink_metadata(path) {
            Ok(metadata) if metadata.file_type().is_file() => {}
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "collector state path has an unsafe file type",
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

pub(crate) fn secure_sqlite_files(root: &Path) -> io::Result<()> {
    preflight_known_paths(root)?;
    for name in [
        "collector.lock",
        "state.sqlite",
        "state.sqlite-wal",
        "state.sqlite-shm",
    ] {
        let path = root.join(name);
        if path.exists() {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        }
    }
    verify_sqlite_files(root)
}

pub(crate) fn verify_sqlite_files(root: &Path) -> io::Result<()> {
    verify_root(root)?;
    preflight_known_paths(root)?;
    let current_uid = effective_uid();
    for name in [
        "collector.lock",
        "state.sqlite",
        "state.sqlite-wal",
        "state.sqlite-shm",
    ] {
        let path = root.join(name);
        if let Ok(metadata) = std::fs::metadata(path)
            && (metadata.uid() != current_uid || metadata.permissions().mode() & 0o777 != 0o600)
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "collector state file permissions are unsafe",
            ));
        }
    }
    Ok(())
}

fn effective_uid() -> u32 {
    // SAFETY: `geteuid` has no preconditions and touches no caller memory.
    unsafe { libc::geteuid() }
}

impl Drop for DataRootLock {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read as _, Write as _},
        os::unix::fs::symlink,
        process::{Command, Stdio},
        thread,
        time::{Duration, Instant},
    };

    const CHILD_LOCK_ROOT: &str = "CLOUD_FMAN_TELEMETRY_TEST_CHILD_LOCK_ROOT";
    const CHILD_READY_PATH: &str = "CLOUD_FMAN_TELEMETRY_TEST_CHILD_READY_PATH";

    #[test]
    fn only_one_process_owner_can_acquire_a_data_root() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("collector");
        let ready = directory.path().join("child-ready");
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "data_root_lock::tests::child_process_holds_data_root_lock",
                "--nocapture",
            ])
            .env(CHILD_LOCK_ROOT, &root)
            .env(CHILD_READY_PATH, &ready)
            .stdin(Stdio::piped())
            .spawn()
            .unwrap();

        let deadline = Instant::now() + Duration::from_secs(10);
        while !ready.exists() {
            assert!(
                Instant::now() < deadline,
                "child process did not acquire the data-root lock"
            );
            assert_eq!(
                child.try_wait().unwrap(),
                None,
                "child process exited early"
            );
            thread::sleep(Duration::from_millis(10));
        }

        let second = match DataRootLock::acquire(&root) {
            Ok(_) => panic!("second lock unexpectedly succeeded"),
            Err(error) => error,
        };
        assert_eq!(second.kind(), io::ErrorKind::WouldBlock);

        child.stdin.take().unwrap().write_all(&[0]).unwrap();
        assert!(child.wait().unwrap().success());
        DataRootLock::acquire(&root).unwrap();
    }

    #[test]
    fn child_process_holds_data_root_lock() {
        let Some(root) = std::env::var_os(CHILD_LOCK_ROOT) else {
            return;
        };
        let ready = std::env::var_os(CHILD_READY_PATH).unwrap();
        let _lock = DataRootLock::acquire(Path::new(&root)).unwrap();
        std::fs::write(ready, []).unwrap();
        std::io::stdin().read_exact(&mut [0]).unwrap();
    }

    #[test]
    fn rejects_unsafe_root_modes_and_state_symlinks() {
        let outer = tempfile::tempdir().unwrap();
        let unsafe_root = outer.path().join("unsafe");
        std::fs::create_dir(&unsafe_root).unwrap();
        std::fs::set_permissions(&unsafe_root, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            DataRootLock::acquire(&unsafe_root).err().unwrap().kind(),
            io::ErrorKind::PermissionDenied
        );

        let safe_root = outer.path().join("safe");
        std::fs::create_dir(&safe_root).unwrap();
        std::fs::set_permissions(&safe_root, std::fs::Permissions::from_mode(0o700)).unwrap();
        symlink("/dev/null", safe_root.join("state.sqlite")).unwrap();
        assert_eq!(
            DataRootLock::acquire(&safe_root).err().unwrap().kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn creates_private_root_and_lock() {
        let outer = tempfile::tempdir().unwrap();
        let root = outer.path().join("collector");
        let _lock = DataRootLock::acquire(&root).unwrap();
        assert_eq!(
            std::fs::metadata(&root).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(root.join("collector.lock"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
}
