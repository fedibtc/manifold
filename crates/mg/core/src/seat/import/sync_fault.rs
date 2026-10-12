use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Mutex;

pub(super) static FAILURES: Mutex<BTreeSet<PathBuf>> = Mutex::new(BTreeSet::new());

pub(crate) struct Guard(PathBuf);

pub(crate) fn at(path: PathBuf) -> Guard {
    assert!(FAILURES.lock().unwrap().insert(path.clone()));
    Guard(path)
}

impl Drop for Guard {
    fn drop(&mut self) {
        FAILURES.lock().unwrap().remove(&self.0);
    }
}
