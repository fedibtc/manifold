use super::*;
use fedi_decentralized_manifold_environment::ManifoldEnvironment;
use fedi_decentralized_service_manifold_guardian::{
    FaId, FederationId, QuoteId, SeatId, SeatTransferGrant, SignedRequest, Timestamp,
};
use mg_core::identity::RootMnemonic;
use std::sync::Arc;

#[tokio::test]
async fn unbound_or_unenrolled_destination_fails_locally_without_dialing() {
    let endpoint = iroh::Endpoint::bind(iroh::endpoint::presets::N0DisableRelay)
        .await
        .unwrap();
    let adapter = IrohTransferSource::new(endpoint.clone());
    let identity = RootMnemonic::generate().unwrap();
    let key = identity.derive_service_signing_key();
    // This source has no address route: any accidental network fallback stalls.
    let source = Locator::new(
        identity.derive_iroh_secret_key().public().into(),
        identity.derive_service_pubkey(),
    );
    let request = MgSignedRequest::create(
        &TransferSeatRequest {
            ts: Timestamp(1_000),
            destination_service_pubkey: key.x_only_public_key().0,
            grant: SignedRequest::create(
                &SeatTransferGrant {
                    ts: Timestamp(1_000),
                    fa_id: FaId(key.x_only_public_key().0),
                    federation_id: FederationId("unbound-test".into()),
                    source: source.clone(),
                    source_seat_id: SeatId::from(QuoteId([41; 32])),
                    destination_service_pubkey: key.x_only_public_key().0,
                    destination_seat_id: SeatId::from(QuoteId([42; 32])),
                },
                &key,
            )
            .unwrap(),
        },
        &key,
    )
    .unwrap();
    assert!(
        tokio::time::timeout(
            Duration::from_secs(1),
            adapter.transfer(&source, request.clone()),
        )
        .await
        .unwrap()
        .is_err()
    );
    let temp = tempfile::TempDir::new().unwrap();
    let db = mg_core::db::Db::open(temp.path()).await.unwrap();
    adapter.bind_proof_source(mg_nostr::ManifoldGuardianNostr::new(
        identity.derive_service_nostr_keys(),
        None,
        Vec::new(),
        None,
        ManifoldEnvironment::Development.profile().unwrap(),
        Arc::new(mg_core::fleet::FleetHolderAuthorizationStore::new(
            db.clone(),
        )),
        db,
    ));
    assert!(
        tokio::time::timeout(Duration::from_secs(1), adapter.transfer(&source, request),)
            .await
            .unwrap()
            .is_err()
    );
    endpoint.close().await;
}
