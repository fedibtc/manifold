use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use fedi_decentralized_service_liquidity_manager::{
    BitcoinNetwork, FederationId, GuardianIdentity, HashBytes, PeerId,
};

use super::*;
use crate::federation_preview::PreviewPeer;

fn fixture_dir(name: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before UNIX epoch")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("flip-trust-fixtures-{name}-{nanos}"));
    std::fs::create_dir_all(&dir).expect("create fixture dir");
    dir
}

fn sample_preview() -> FederationPreview {
    FederationPreview {
        federation_id: FederationId("fed-1".to_owned()),
        federation_config_hash: HashBytes(vec![7; 32]),
        network: BitcoinNetwork::Regtest,
        peers: vec![PreviewPeer {
            peer_id: PeerId("0".to_owned()),
            guardian_identity: GuardianIdentity("guardian-0".to_owned()),
        }],
        consensus_threshold: 3,
        fman_seat_bindings_metadata: Some("fman-api-urls-v1 iroh://fman-1".to_owned()),
        module_kinds: [
            "wallet",
            "mint",
            crate::stability_pool::STABILITY_POOL_MODULE_KIND,
        ]
        .into_iter()
        .map(str::to_owned)
        .collect(),
    }
}

fn write_previews(dir: &Path, previews: &HashMap<String, FederationPreview>) {
    std::fs::write(
        dir.join(PREVIEWS_FIXTURE_FILENAME),
        serde_json::to_string(previews).expect("serialize previews fixture"),
    )
    .expect("write previews fixture");
}

#[tokio::test]
async fn preview_round_trips_through_fixture_file() {
    let dir = fixture_dir("preview-round-trip");
    let preview = sample_preview();
    write_previews(
        &dir,
        &HashMap::from([("fed11invite".to_owned(), preview.clone())]),
    );

    let provider = FixtureFederationPreviewProvider::new(dir);
    let loaded = provider
        .preview(&InviteCode("fed11invite".to_owned()))
        .await
        .expect("fixture preview loads");
    assert_eq!(loaded, preview);
}

#[tokio::test]
async fn preview_reads_lazily_per_call() {
    // The daemon may boot before the fixture content exists; a call after
    // the file appears must succeed without any restart.
    let dir = fixture_dir("preview-lazy");
    let provider = FixtureFederationPreviewProvider::new(dir.clone());
    let invite = InviteCode("fed11invite".to_owned());

    let missing_file = provider
        .preview(&invite)
        .await
        .expect_err("no fixture file");
    assert!(matches!(
        missing_file,
        FederationPreviewError::Unavailable(_)
    ));

    write_previews(
        &dir,
        &HashMap::from([("fed11invite".to_owned(), sample_preview())]),
    );
    provider
        .preview(&invite)
        .await
        .expect("fixture preview loads after the file appears");
}

#[tokio::test]
async fn preview_missing_entry_and_malformed_file_are_unavailable() {
    let dir = fixture_dir("preview-errors");
    let provider = FixtureFederationPreviewProvider::new(dir.clone());
    write_previews(&dir, &HashMap::new());

    let missing_entry = provider
        .preview(&InviteCode("unknown".to_owned()))
        .await
        .expect_err("missing entry");
    assert!(
        matches!(&missing_entry, FederationPreviewError::Unavailable(reason)
            if reason.contains("no fixture preview"))
    );

    std::fs::write(dir.join(PREVIEWS_FIXTURE_FILENAME), "not json")
        .expect("write malformed fixture");
    let malformed = provider
        .preview(&InviteCode("unknown".to_owned()))
        .await
        .expect_err("malformed file");
    assert!(
        matches!(&malformed, FederationPreviewError::Unavailable(reason)
            if reason.contains("failed to parse"))
    );
}
