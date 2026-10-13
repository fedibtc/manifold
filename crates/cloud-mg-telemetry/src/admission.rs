use fedi_decentralized_service_manifold_guardian::MgName;
use nostr::PublicKey;

pub(crate) fn display_name(signer: &str) -> Result<String, BadgeRefused> {
    let key = PublicKey::parse(signer).map_err(|_| BadgeRefused)?;
    Ok(MgName::from_mg_id(key).to_string())
}

/// Uniform badge refusal without envelope detail.
#[derive(Debug, thiserror::Error)]
#[error("registration credential refused")]
pub(crate) struct BadgeRefused;
