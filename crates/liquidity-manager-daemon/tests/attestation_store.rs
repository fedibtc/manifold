use super::*;
use crate::test_support::credentials::{
    UNIT_TEST_ISSUER_RELAY, attestation_payload, test_issuer_authority, test_issuer_context,
};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

#[tokio::test]
async fn installs_lists_and_removes_issuer_authority() -> anyhow::Result<()> {
    let database = test_database("issuer-authority").await?;
    let issuer = test_issuer_context();
    let authority = test_issuer_authority(&issuer, UNIT_TEST_ISSUER_RELAY)?;

    let response = install(
        &database,
        AttestationInstallRequest {
            payload: attestation_payload(&authority)?,
        },
    )
    .await?;

    assert_eq!(response.kind, AttestationKind::IssuerAuthority);

    let summary = summary(&database).await?;
    assert_eq!(summary.issuer_authorities, 1);
    assert_eq!(summary.valid, 1);
    assert_eq!(summary.invalid, 0);

    let listed = list(&database).await?;
    assert_eq!(listed.payloads.len(), 1);
    assert_eq!(listed.payloads[0].id, response.id);
    assert_eq!(listed.payloads[0].kind, AttestationKind::IssuerAuthority);
    assert!(listed.payloads[0].valid);

    remove(
        &database,
        AttestationRemoveRequest {
            target: AttestationSelector::Id(response.id),
        },
    )
    .await?;
    assert!(list(&database).await?.payloads.is_empty());

    Ok(())
}

async fn test_database(name: &str) -> anyhow::Result<Database> {
    Database::connect(test_data_dir(name).join("flip.sqlite")).await
}

fn test_data_dir(name: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before UNIX epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("flip-attestation-store-{name}-{nanos}"))
}
