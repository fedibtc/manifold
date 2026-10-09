use std::{
    fs::{File, OpenOptions},
    io::Write as _,
    path::Path,
};

use fedi_decentralized_service_peerbadge_signing::SigningError;
use serde::Serialize;

#[derive(Serialize)]
pub(crate) struct AuditEvent<'a> {
    pub ts: u64,
    pub event: &'static str,
    pub signer_pubkey: Option<&'a str>,
    pub level: Option<u8>,
    pub session_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_sha256: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<&'static str>,
}

pub(crate) struct Audit {
    file: File,
    failed: bool,
}

impl Audit {
    pub fn open(path: &Path) -> std::io::Result<Self> {
        let mut options = OpenOptions::new();
        options.create(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        let file = options.open(path)?;
        Ok(Self {
            file,
            failed: false,
        })
    }

    pub fn check(&self) -> Result<(), SigningError> {
        if self.failed {
            Err(unavailable())
        } else {
            Ok(())
        }
    }

    pub fn record(&mut self, event: AuditEvent<'_>) -> Result<(), SigningError> {
        self.check()?;
        let mut line = serde_json::to_vec(&event).map_err(|_| unavailable())?;
        line.push(b'\n');
        // The mutex serializes complete lines and mutation commits. Once any
        // write fails, no operation may succeed until an operator restarts.
        if self
            .file
            .write_all(&line)
            .and_then(|()| self.file.sync_data())
            .is_err()
        {
            self.failed = true;
            return Err(unavailable());
        }
        Ok(())
    }
}

pub(crate) fn unavailable() -> SigningError {
    SigningError::Transport("signing service unavailable".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn audit_capabilities_are_not_world_readable() {
        use std::os::unix::fs::PermissionsExt as _;

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("audit.jsonl");
        let _audit = Audit::open(&path).unwrap();
        assert_eq!(
            std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn io_failure_is_sticky_and_never_acknowledged() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("audit.jsonl");
        std::fs::write(&path, "").unwrap();
        // A read-only descriptor fails writes on macOS and Linux, even as root.
        let mut audit = Audit {
            file: File::open(&path).unwrap(),
            failed: false,
        };
        let result = audit.record(AuditEvent {
            ts: 1,
            event: "session_opened",
            signer_pubkey: None,
            level: Some(9),
            session_id: Some("test"),
            request_sha256: None,
            reason: None,
        });
        assert!(result.is_err());
        assert!(audit.check().is_err());
        audit.file = OpenOptions::new().append(true).open(&path).unwrap();
        assert!(audit.check().is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"");
    }
}
