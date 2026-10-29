//! On-disk archive of one federation's session outcomes.
//!
//! Layout of the archive directory:
//!
//! - `federation.json`: the federation id and the instance id of each module
//!   kind, written once from the threshold-agreed client config.
//! - `sessions/<index>.bin`: the consensus encoding of one `SessionOutcome`.
//!
//! Each file is written to a temporary name and renamed, so a crash never
//! leaves a partial session behind.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, ensure};
use fedimint_core::config::ClientConfig;
use fedimint_core::core::{ModuleInstanceId, ModuleKind};
use fedimint_core::encoding::{Decodable as _, Encodable as _};
use fedimint_core::module::CommonModuleInit as _;
use fedimint_core::module::registry::ModuleDecoderRegistry;
use fedimint_core::session_outcome::SessionOutcome;
use fedimint_lnv2_common::LightningCommonInit;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FederationInfo {
    pub federation_id: String,
    /// Module kind by instance id, for example `0 -> lnv2`.
    pub modules: BTreeMap<ModuleInstanceId, String>,
}

impl FederationInfo {
    pub fn from_config(config: &ClientConfig) -> Self {
        Self {
            federation_id: config.calculate_federation_id().to_string(),
            modules: config
                .modules
                .iter()
                .map(|(id, module)| (*id, module.kind.to_string()))
                .collect(),
        }
    }

    pub fn lnv2_instance(&self) -> Option<ModuleInstanceId> {
        self.modules
            .iter()
            .find(|(_, kind)| kind.as_str() == "lnv2")
            .map(|(id, _)| *id)
    }

    /// Decoders for the modules this tool understands. Every other module
    /// decodes as an opaque item and re-encodes byte for byte.
    pub fn decoders(&self) -> ModuleDecoderRegistry {
        ModuleDecoderRegistry::from_iter(self.lnv2_instance().map(|id| {
            (
                id,
                ModuleKind::from_static_str("lnv2"),
                LightningCommonInit::decoder(),
            )
        }))
        .with_fallback()
    }
}

pub struct Archive {
    dir: PathBuf,
}

impl Archive {
    pub fn new(dir: &Path) -> Self {
        Self {
            dir: dir.to_path_buf(),
        }
    }

    fn info_path(&self) -> PathBuf {
        self.dir.join("federation.json")
    }

    fn sessions_dir(&self) -> PathBuf {
        self.dir.join("sessions")
    }

    fn session_path(&self, index: u64) -> PathBuf {
        self.sessions_dir().join(format!("{index}.bin"))
    }

    /// Record the federation on first use, and refuse an archive that
    /// belongs to a different federation.
    pub fn init(&self, info: &FederationInfo) -> anyhow::Result<()> {
        std::fs::create_dir_all(self.sessions_dir())
            .with_context(|| format!("create {}", self.sessions_dir().display()))?;
        match self.info_opt()? {
            Some(existing) => ensure!(
                existing.federation_id == info.federation_id,
                "{} holds federation {}, not {}",
                self.dir.display(),
                existing.federation_id,
                info.federation_id
            ),
            None => write_atomic(&self.info_path(), &serde_json::to_vec_pretty(info)?)?,
        }
        Ok(())
    }

    fn info_opt(&self) -> anyhow::Result<Option<FederationInfo>> {
        match std::fs::read(self.info_path()) {
            Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e).context("read federation.json"),
        }
    }

    pub fn info(&self) -> anyhow::Result<FederationInfo> {
        self.info_opt()?
            .with_context(|| format!("{} is not an archive: run `sync` first", self.dir.display()))
    }

    pub fn has(&self, index: u64) -> bool {
        self.session_path(index).exists()
    }

    pub fn store(&self, index: u64, outcome: &SessionOutcome) -> anyhow::Result<()> {
        write_atomic(
            &self.session_path(index),
            &outcome.consensus_encode_to_vec(),
        )
    }

    pub fn load(
        &self,
        index: u64,
        decoders: &ModuleDecoderRegistry,
    ) -> anyhow::Result<Option<SessionOutcome>> {
        let bytes = match std::fs::read(self.session_path(index)) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e).with_context(|| format!("read session {index}")),
        };
        let outcome = SessionOutcome::consensus_decode_whole(&bytes, decoders)
            .with_context(|| format!("decode session {index}"))?;
        Ok(Some(outcome))
    }

    /// Indices of all stored sessions, ascending.
    pub fn indices(&self) -> anyhow::Result<Vec<u64>> {
        let mut out = vec![];
        for entry in std::fs::read_dir(self.sessions_dir())? {
            let name = entry?.file_name();
            if let Some(index) = name
                .to_str()
                .and_then(|n| n.strip_suffix(".bin"))
                .and_then(|n| n.parse().ok())
            {
                out.push(index);
            }
        }
        out.sort_unstable();
        Ok(out)
    }
}

fn write_atomic(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes).with_context(|| format!("write {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("rename to {}", path.display()))?;
    Ok(())
}
