//! File-based trust-input fixtures for test deployments.
//!
//! `--trust-fixtures <dir>` substitutes exactly one verification input whose
//! real implementation is a tracked open item: the Fedimint invite-code
//! preview. Every other stage of the verification pipeline (signature checks,
//! seat binding, credential and revocation verification, policy evaluation)
//! runs unchanged. FMan trust standing arrives as signed material inside the
//! request and is never substitutable, and the revocation fetch always uses
//! the real relay path. Fixture mode is refused for Bitcoin mainnet (see
//! `setup_store::ensure_trust_fixtures_allow_network`).
//!
//! Fixture files are read lazily on every call: live harnesses boot the
//! daemon before the target federation (and thus the fixture content)
//! exists.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use fedi_decentralized_service_liquidity_manager::InviteCode;
use serde::de::DeserializeOwned;

use crate::federation_preview::{
    FederationPreview, FederationPreviewError, FederationPreviewProvider,
};

/// Fixture file mapping invite code -> [`FederationPreview`].
pub const PREVIEWS_FIXTURE_FILENAME: &str = "previews.json";

async fn read_fixture_map<T: DeserializeOwned>(
    dir: &Path,
    filename: &str,
) -> Result<HashMap<String, T>, String> {
    let path = dir.join(filename);
    let raw = tokio::fs::read_to_string(&path)
        .await
        .map_err(|error| format!("failed to read trust fixture file {path:?}: {error}"))?;
    serde_json::from_str(&raw)
        .map_err(|error| format!("failed to parse trust fixture file {path:?}: {error}"))
}

/// Federation preview provider backed by `<dir>/previews.json`.
#[derive(Clone, Debug)]
pub struct FixtureFederationPreviewProvider {
    dir: PathBuf,
}

impl FixtureFederationPreviewProvider {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }
}

#[async_trait]
impl FederationPreviewProvider for FixtureFederationPreviewProvider {
    async fn preview(
        &self,
        invite_code: &InviteCode,
    ) -> Result<FederationPreview, FederationPreviewError> {
        let previews: HashMap<String, FederationPreview> =
            read_fixture_map(&self.dir, PREVIEWS_FIXTURE_FILENAME)
                .await
                .map_err(FederationPreviewError::Unavailable)?;
        previews.get(&invite_code.0).cloned().ok_or_else(|| {
            FederationPreviewError::Unavailable(format!(
                "no fixture preview for this invite code in {:?}",
                self.dir.join(PREVIEWS_FIXTURE_FILENAME)
            ))
        })
    }
}

#[cfg(test)]
#[path = "../tests/trust_fixtures.rs"]
mod tests;
