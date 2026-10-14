use super::*;
use fedi_decentralized_domain::{MG_SEAT_BINDINGS_META_FIELD_KEY, MgPeerAttestation};
use fedi_decentralized_service_manifold_guardian::{
    canonical_guardian_fee_recipient_list, rebind_transferred_seat,
};

struct BindingFixture {
    _root: TempDir,
    fleet: Fleet,
    seat: Arc<Seat>,
    fake: FakeSeatChildHandle,
    original: Vec<u8>,
    binding: FormationSeatBinding,
    destination: nostr_sdk::Keys,
}

fn endpoint_binding(attestation: MgPeerAttestation) -> FormationSeatBinding {
    let peer = attestation.attestation.peer_id.0.parse::<u8>().unwrap();
    let signature = iroh_base_035::SecretKey::from_bytes(&[0x60 + peer; 32]).sign(
        &attestation
            .attestation
            .seat_endpoint_proof_message()
            .unwrap(),
    );
    FormationSeatBinding {
        attestation,
        endpoint_proof: fedi_decentralized_domain::SeatEndpointProof {
            signature: signature.to_bytes().to_vec(),
        },
    }
}

fn resign(binding: &mut FormationSeatBinding, keys: &nostr_sdk::Keys) {
    binding.attestation.proof.signature =
        keys.sign_schnorr(&nostr_sdk::secp256k1::Message::from_digest(
            binding.attestation.attestation.digest().unwrap(),
        ));
    *binding = endpoint_binding(binding.attestation.clone());
}

async fn binding_fixture(port: u16, moved_peer: usize) -> BindingFixture {
    let root = TempDir::new().unwrap();
    let fleet = open_fleet(config(&root, 1, port).await, Arc::new(NoWallet))
        .await
        .unwrap();
    let (_, seat_id) = create_free_seat(&fleet, 210).await;
    let seat = fleet.seat_by_id(&seat_id).unwrap();
    let config = fixture_client_config();
    let own = fleet.identity().derive_service_nostr_keys();
    let directory = if moved_peer == 0 {
        fixture_directory(&config)
    } else {
        fixture_directory_bound_to(&config, 0, &own)
    };
    let destination = if moved_peer == 0 {
        own
    } else {
        peer_mg_keys(8)
    };
    let binding = endpoint_binding(peer_binding_signed_by(
        &config,
        moved_peer,
        &destination,
        fee_account(0x55),
    ));
    let mut recipients = guardian_fee_recipients(&fee_account(0x20));
    // Transfer preserves the live split; it must not reconstruct formation's
    // default weights or overwrite FA/verification recipients.
    for (index, recipient) in recipients.iter_mut().enumerate() {
        recipient.weight = 11 + index as u64;
    }
    let recipients = canonical_guardian_fee_recipient_list(&recipients).unwrap();
    let original = serde_json_canonicalizer::to_vec(&serde_json::json!({
        MG_SEAT_BINDINGS_META_FIELD_KEY: directory,
        crate::guardian_fee::REMITTANCE_ACCOUNT_META_KEY: recipients,
        crate::guardian_fee::SEND_PPM_META_KEY: "4321",
        "unrelated": {"array": [7, true, "preserve"], "nested": {"value": "untouched"}},
    }))
    .unwrap();
    let fake = fleet
        .form_fake_child(&seat_id, running_federation(0, Some(original.clone())))
        .await;
    BindingFixture {
        _root: root,
        fleet,
        seat,
        fake,
        original,
        binding,
        destination,
    }
}

impl BindingFixture {
    fn base(&self) -> MetaConsensusBase {
        MetaConsensusBase::from_consensus(Some((0, &self.original)))
    }
}

#[tokio::test]
async fn transfer_binding_preserves_whole_object_and_replays_an_ambiguous_vote() {
    let f = binding_fixture(32_209, 1).await;
    f.fake
        .modify_state(|state| state.fail_meta_submit_after_record_once = true);
    assert!(
        f.seat
            .replace_seat_binding(f.base(), f.binding.clone())
            .await
            .is_err()
    );
    let target = f.fake.state().meta_submissions[0].clone();
    assert_eq!(
        f.fake.state().meta_consensus,
        Some(f.original.clone()),
        "one vote is not consensus"
    );
    let before: serde_json::Value = serde_json::from_slice(&f.original).unwrap();
    let mut expected = before.clone();
    let (directory, recipients) = rebind_transferred_seat(
        before[MG_SEAT_BINDINGS_META_FIELD_KEY].as_str().unwrap(),
        before[crate::guardian_fee::REMITTANCE_ACCOUNT_META_KEY]
            .as_str()
            .unwrap(),
        &f.binding.attestation,
    )
    .unwrap();
    expected[MG_SEAT_BINDINGS_META_FIELD_KEY] = directory.into();
    expected[crate::guardian_fee::REMITTANCE_ACCOUNT_META_KEY] = recipients.into();
    assert_eq!(target, serde_json_canonicalizer::to_vec(&expected).unwrap());

    assert!(matches!(
        f.seat
            .submit_meta_field(
                f.base(),
                MetaFieldKey(FEDERATION_NAME_META_FIELD_KEY.into()),
                MetaFieldValue("conflict".into()),
                NO_FEE_FLOOR
            )
            .await,
        Err(SeatVerbError::MetaTargetConflict)
    ));
    f.seat
        .replace_seat_binding(f.base(), f.binding.clone())
        .await
        .unwrap();
    assert_eq!(
        f.fake.state().meta_submissions,
        vec![target.clone(), target.clone()]
    );
    let revision = f.fake.set_meta_consensus(Some(target.clone()));
    assert!(matches!(
        f.seat
            .replace_seat_binding(f.base(), f.binding.clone())
            .await,
        Err(SeatVerbError::MetaConsensusChanged)
    ));
    // The same helper target is idempotent even after consensus adoption.
    f.seat
        .replace_seat_binding(
            MetaConsensusBase::from_consensus(Some((revision, &target))),
            f.binding.clone(),
        )
        .await
        .unwrap();
    assert_eq!(f.fake.state().meta_submissions.last(), Some(&target));
    f.fleet.shutdown().await;
}

#[tokio::test]
async fn transfer_binding_destination_can_vote_for_its_own_moved_peer() {
    let f = binding_fixture(32_213, 0).await;
    f.seat
        .replace_seat_binding(f.base(), f.binding.clone())
        .await
        .unwrap();
    assert_eq!(f.fake.state().meta_submissions.len(), 1);
    f.fleet.shutdown().await;
}

#[tokio::test]
async fn transfer_binding_refuses_forged_or_wrong_native_identity_without_pinning() {
    let f = binding_fixture(32_217, 1).await;
    let mut forged = f.binding.clone();
    forged.attestation.attestation.issued_at.0 += 1;
    let mut wrong_guardian = f.binding.clone();
    wrong_guardian.attestation.attestation.guardian_identity =
        peer_binding(&fixture_client_config(), 2, fee_account(0x22))
            .attestation
            .guardian_identity;
    resign(&mut wrong_guardian, &f.destination);
    let mut wrong_federation = fixture_client_config();
    wrong_federation.global.consensus_version =
        fedimint_core::module::CoreConsensusVersion::new(2, 0);
    let wrong_federation = endpoint_binding(peer_binding_signed_by(
        &wrong_federation,
        1,
        &f.destination,
        fee_account(0x55),
    ));
    let mut wrong_endpoint = f.binding.clone();
    wrong_endpoint.endpoint_proof.signature = iroh_base_035::SecretKey::from_bytes(&[0x7f; 32])
        .sign(
            &wrong_endpoint
                .attestation
                .attestation
                .seat_endpoint_proof_message()
                .unwrap(),
        )
        .to_bytes()
        .to_vec();
    let duplicate_fee = endpoint_binding(peer_binding_signed_by(
        &fixture_client_config(),
        1,
        &f.destination,
        fee_account(0x22),
    ));
    for invalid in [
        forged,
        wrong_guardian,
        wrong_federation,
        wrong_endpoint,
        duplicate_fee,
    ] {
        assert!(matches!(
            f.seat.replace_seat_binding(f.base(), invalid).await,
            Err(SeatVerbError::MetaValueInvalid)
        ));
    }
    assert!(f.fake.state().meta_submissions.is_empty());
    f.seat
        .replace_seat_binding(f.base(), f.binding.clone())
        .await
        .unwrap();
    assert_eq!(
        f.fake.state().meta_submissions.len(),
        1,
        "refusals did not pin the base"
    );
    f.fleet.shutdown().await;
}

#[tokio::test]
async fn transfer_binding_shares_admission_with_ordinary_metadata_and_bounds_target() {
    let mut f = binding_fixture(32_221, 1).await;
    f.seat
        .submit_meta_field(
            f.base(),
            MetaFieldKey(FEDERATION_NAME_META_FIELD_KEY.into()),
            MetaFieldValue("first".into()),
            NO_FEE_FLOOR,
        )
        .await
        .unwrap();
    assert!(matches!(
        f.seat
            .replace_seat_binding(f.base(), f.binding.clone())
            .await,
        Err(SeatVerbError::MetaTargetConflict)
    ));
    let mut fields: serde_json::Value = serde_json::from_slice(&f.original).unwrap();
    fields["padding"] = "".into();
    let unpadded = serde_json_canonicalizer::to_vec(&fields).unwrap().len();
    fields["padding"] = "x"
        .repeat(FEDERATION_METADATA_OBJECT_MAX_BYTES - unpadded)
        .into();
    let at_cap = serde_json_canonicalizer::to_vec(&fields).unwrap();
    assert_eq!(at_cap.len(), FEDERATION_METADATA_OBJECT_MAX_BYTES);
    let revision = f.fake.set_meta_consensus(Some(at_cap.clone()));
    f.binding.attestation.attestation.issued_at.0 = u64::MAX;
    resign(&mut f.binding, &f.destination);
    assert!(matches!(
        f.seat
            .replace_seat_binding(
                MetaConsensusBase::from_consensus(Some((revision, &at_cap))),
                f.binding.clone()
            )
            .await,
        Err(SeatVerbError::MetaValueInvalid)
    ));
    assert_eq!(
        f.fake.state().meta_submissions.len(),
        1,
        "oversized target never reaches the child"
    );
    f.fleet.shutdown().await;
}
