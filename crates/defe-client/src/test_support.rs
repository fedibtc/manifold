use std::fs;
use std::os::unix::fs::DirBuilderExt as _;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU16, Ordering};

pub(crate) fn unique_temp_dir(name: &str) -> PathBuf {
    static NEXT_ID: AtomicU16 = AtomicU16::new(0);
    for _ in 0..=u8::MAX {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "c{:04x}{:02x}",
            std::process::id() & 0xffff,
            id & 0xff
        ));
        let mut builder = fs::DirBuilder::new();
        builder.mode(0o700);
        match builder.create(&path) {
            Ok(()) => return path,
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(err) => panic!("create {name} test directory {}: {err}", path.display()),
        }
    }

    panic!("exhausted compact temp directory names for {name}")
}
