use super::*;

fn maintenance_options() -> MaintenanceRunOptions {
    MaintenanceRunOptions::new(MaintenanceRunOptionsConfig {
        poll_interval: Duration::from_millis(1),
        run_timeout: Duration::from_secs(2),
        request_timeout: Duration::from_millis(200),
    })
    .unwrap()
}

fn long_maintenance_options() -> MaintenanceRunOptions {
    MaintenanceRunOptions::new(MaintenanceRunOptionsConfig {
        poll_interval: Duration::from_millis(1),
        run_timeout: Duration::from_secs(2),
        request_timeout: Duration::from_secs(30),
    })
    .unwrap()
}

#[test]
fn maintenance_timing_errors_use_maintenance_vocabulary() {
    let error = MaintenanceRunOptions::new(MaintenanceRunOptionsConfig {
        poll_interval: Duration::ZERO,
        run_timeout: Duration::from_millis(1),
        request_timeout: Duration::from_millis(1),
    })
    .unwrap_err();
    assert_eq!(
        error,
        InvalidMaintenanceRunOptions::BelowMinimum {
            field: MaintenanceTimingField::PollInterval,
        }
    );
    assert_eq!(
        error.to_string(),
        "invalid maintenance options: poll interval must be at least one millisecond"
    );
}

#[test]
fn maintenance_backoff_never_shortens_the_public_poll_interval() {
    assert_eq!(
        first_three_maintenance_retry_delays(Duration::from_secs(2)),
        [
            Duration::from_secs(2),
            Duration::from_secs(4),
            Duration::from_secs(5),
        ]
    );
    assert_eq!(
        first_three_maintenance_retry_delays(Duration::from_secs(5)),
        [Duration::from_secs(5); 3]
    );
    assert_eq!(
        first_three_maintenance_retry_delays(Duration::from_secs(7)),
        [Duration::from_secs(7); 3],
        "the private backoff cap cannot make a caller-selected interval more aggressive"
    );
}

#[test]
fn public_variants_map_exactly_and_invalid_values_exist_before_driver_work() {
    for (update, key, value) in [
        (
            FederationMetadataUpdate::name("New Federation").unwrap(),
            FEDERATION_NAME_META_FIELD_KEY,
            "New Federation",
        ),
        (
            FederationMetadataUpdate::icon_url("https://example.com/icon.png").unwrap(),
            FEDERATION_ICON_URL_META_FIELD_KEY,
            "https://example.com/icon.png",
        ),
        (
            FederationMetadataUpdate::welcome_message("Hello members").unwrap(),
            WELCOME_MESSAGE_META_FIELD_KEY,
            "Hello members",
        ),
        (
            FederationMetadataUpdate::terms_of_service_url("  https://example.com/custom-terms  ")
                .unwrap(),
            TERMS_OF_SERVICE_URL_META_FIELD_KEY,
            "  https://example.com/custom-terms  ",
        ),
    ] {
        let (actual_key, actual_value) = update.into_field();
        assert_eq!(actual_key.0, key);
        assert_eq!(actual_value.0, value);
    }

    assert!(FederationMetadataUpdate::name("no").is_err());
    assert!(FederationMetadataUpdate::icon_url("data:image/png;base64,AA==").is_err());
    assert!(FederationMetadataUpdate::welcome_message("").is_err());
    assert!(
        FederationMetadataUpdate::welcome_message("x".repeat(
            fedi_decentralized_service_manifold_guardian::FEDERATION_METADATA_RAW_MAX_BYTES + 1
        ))
        .is_err()
    );
}

#[tokio::test]
async fn oversized_update_cannot_reach_the_driver_or_network() {
    let (payments, _) = TestPayments::new();
    let mg_state = Arc::new(MgState::default());
    let _client = open_client(
        MemDatabase::new().into_database(),
        payments,
        mg_state.clone(),
        MgConfig::given_away(),
    )
    .await;

    assert!(
        FederationMetadataUpdate::welcome_message("x".repeat(
            fedi_decentralized_service_manifold_guardian::FEDERATION_METADATA_RAW_MAX_BYTES + 1
        ))
        .is_err()
    );
    assert_eq!(mg_state.connect_calls.load(Ordering::SeqCst), 0);
    assert!(
        mg_state
            .meta_request_bases
            .lock()
            .expect("test lock")
            .is_empty()
    );
}

#[tokio::test]
async fn pre_formed_rejection_has_no_connector_or_metadata_effect() {
    let database = MemDatabase::new().into_database();
    let (payments, _) = TestPayments::new();
    let mg_state = Arc::new(MgState::default());
    let client = open_client(database, payments, mg_state.clone(), MgConfig::given_away()).await;
    client
        .inner
        .store
        .initialize_for_test(
            TestIdentity::fa_id(),
            FormationId("maintenance-pre-formed".to_owned()),
            resolved_intent_with_size(FederationSize(1)),
            vec![selected_initial_seat(0, test_now_secs() + 120)],
            None,
        )
        .await
        .unwrap();

    assert!(matches!(
        client
            .update_federation_metadata(
                FederationMetadataUpdate::name("New Federation").unwrap(),
                maintenance_options(),
            )
            .await,
        Err(FaError::NoActiveFormation)
    ));
    assert_eq!(mg_state.connect_calls.load(Ordering::SeqCst), 0);
    assert!(
        mg_state
            .meta_request_bases
            .lock()
            .expect("test lock")
            .is_empty()
    );
}

#[tokio::test]
async fn gateway_registration_fans_out_and_accepts_a_threshold_live_federation() {
    let database = MemDatabase::new().into_database();
    let (payments, _) = TestPayments::new();
    let mg_state = Arc::new(MgState::default());
    let reader = TestConsensusReader::new(mg_state.clone());
    let client = open_client_with_reader(
        database,
        payments,
        mg_state.clone(),
        MgConfig::given_away(),
        reader,
    )
    .await;
    client
        .buy_and_form_at(intent(), locators(), options())
        .await
        .unwrap();

    *mg_state.offline_indices.lock().expect("test lock") = [0, 1].into_iter().collect();
    let gateway_api = GatewayApiUrl::try_from(
        "iroh://8a88e3dd7409f195fd52db2d3cba5d72ca6709bf1d94121bf3748801b40f6f5c",
    )
    .unwrap();
    client
        .register_gateway(gateway_api.clone(), maintenance_options())
        .await
        .unwrap();

    let registrations = mg_state.gateway_registrations.lock().expect("test lock");
    assert_eq!(
        registrations.len(),
        test_federation_seats().consensus_threshold() as usize
    );
    assert!(
        registrations
            .iter()
            .all(|(_, url)| url == gateway_api.as_str())
    );
}

#[tokio::test]
async fn terms_url_preserves_unrelated_fields_and_waits_for_consensus() {
    let database = MemDatabase::new().into_database();
    let (payments, _) = TestPayments::new();
    let mg_state = Arc::new(MgState::default());
    let reader = TestConsensusReader::new(mg_state.clone());
    let client = open_client_with_reader(
        database,
        payments,
        mg_state.clone(),
        MgConfig::given_away(),
        reader,
    )
    .await;
    client
        .buy_and_form_at(intent(), locators(), options())
        .await
        .unwrap();

    mg_state.meta_submissions.lock().expect("test lock").clear();
    mg_state
        .meta_request_bases
        .lock()
        .expect("test lock")
        .clear();
    let before = mg_state
        .meta_consensus_raw
        .lock()
        .expect("test lock")
        .clone()
        .expect("formation published metadata");
    client
        .update_federation_metadata(
            FederationMetadataUpdate::terms_of_service_url("https://example.com/custom-terms")
                .unwrap(),
            maintenance_options(),
        )
        .await
        .unwrap();

    let after = mg_state
        .meta_consensus_raw
        .lock()
        .expect("test lock")
        .clone()
        .expect("maintenance published metadata");
    let mut expected: BTreeMap<String, serde_json::Value> =
        serde_json::from_slice(&before).expect("prior metadata parses");
    let after: BTreeMap<String, serde_json::Value> =
        serde_json::from_slice(&after).expect("updated metadata parses");
    expected.insert(
        TERMS_OF_SERVICE_URL_META_FIELD_KEY.to_owned(),
        serde_json::Value::String("https://example.com/custom-terms".to_owned()),
    );
    assert_eq!(after, expected);
    assert_eq!(
        mg_state.meta_submissions.lock().expect("test lock").len(),
        usize::from(MIN_FEDERATION_SIZE)
    );
}

#[tokio::test]
async fn rebases_the_identical_mutation_after_a_stale_wave() {
    let database = MemDatabase::new().into_database();
    let (payments, _) = TestPayments::new();
    let mg_state = Arc::new(MgState::default());
    let reader = TestConsensusReader::new(mg_state.clone());
    let client = open_client_with_reader(
        database,
        payments,
        mg_state.clone(),
        MgConfig::given_away(),
        reader.clone(),
    )
    .await;
    client
        .buy_and_form_at(intent(), locators(), options())
        .await
        .unwrap();

    mg_state.meta_submissions.lock().expect("test lock").clear();
    mg_state
        .meta_request_bases
        .lock()
        .expect("test lock")
        .clear();
    let initial = serde_json::to_vec(&serde_json::json!({"existing": "old"})).unwrap();
    let concurrent = serde_json::to_vec(&serde_json::json!({"existing": "new"})).unwrap();
    reader.change_base_after_next_read(initial.clone(), concurrent.clone());
    client
        .update_federation_metadata(
            FederationMetadataUpdate::welcome_message("Hello members").unwrap(),
            maintenance_options(),
        )
        .await
        .unwrap();

    let bases = mg_state.meta_request_bases.lock().expect("test lock");
    let seat_count = usize::from(MIN_FEDERATION_SIZE);
    assert_eq!(bases.len(), seat_count * 2);
    // The concurrent adoption bumped the fake consensus revision 0 -> 1.
    assert!(
        bases[..seat_count]
            .iter()
            .all(|(_, base)| *base == MetaConsensusBase::from_consensus(Some((0, &initial))))
    );
    assert!(
        bases[seat_count..]
            .iter()
            .all(|(_, base)| *base == MetaConsensusBase::from_consensus(Some((1, &concurrent))))
    );
    let final_raw = mg_state
        .meta_consensus_raw
        .lock()
        .expect("test lock")
        .clone()
        .expect("maintenance published metadata");
    let final_fields: BTreeMap<String, serde_json::Value> =
        serde_json::from_slice(&final_raw).expect("updated metadata parses");
    assert_eq!(
        final_fields.get("existing"),
        Some(&serde_json::Value::String("new".to_owned()))
    );
    assert_eq!(
        final_fields.get(WELCOME_MESSAGE_META_FIELD_KEY),
        Some(&serde_json::Value::String("Hello members".to_owned()))
    );
}

#[tokio::test]
async fn rebases_across_a_revision_bump_with_unchanged_content() {
    let database = MemDatabase::new().into_database();
    let (payments, _) = TestPayments::new();
    let mg_state = Arc::new(MgState::default());
    let reader = TestConsensusReader::new(mg_state.clone());
    let client = open_client_with_reader(
        database,
        payments,
        mg_state.clone(),
        MgConfig::given_away(),
        reader.clone(),
    )
    .await;
    client
        .buy_and_form_at(intent(), locators(), options())
        .await
        .unwrap();

    mg_state.meta_submissions.lock().expect("test lock").clear();
    mg_state
        .meta_request_bases
        .lock()
        .expect("test lock")
        .clear();
    // A do/undo pair adopted elsewhere returned the board to byte-identical
    // content under a fresh revision. The FA's first wave is signed against
    // the old occurrence; guardians refuse it as stale, and the rebase binds
    // the identical mutation to the fresh occurrence of the same bytes.
    let recurred = serde_json::to_vec(&serde_json::json!({"existing": "recurred"})).unwrap();
    reader.bump_revision_after_next_read(recurred.clone(), 2);
    client
        .update_federation_metadata(
            FederationMetadataUpdate::welcome_message("Hello again").unwrap(),
            maintenance_options(),
        )
        .await
        .unwrap();

    let bases = mg_state.meta_request_bases.lock().expect("test lock");
    let seat_count = usize::from(MIN_FEDERATION_SIZE);
    assert_eq!(bases.len(), seat_count * 2, "one stale wave and one rebase");
    let old_occurrence = MetaConsensusBase::from_consensus(Some((0, &recurred)));
    let new_occurrence = MetaConsensusBase::from_consensus(Some((2, &recurred)));
    assert_ne!(
        old_occurrence, new_occurrence,
        "identical bytes under a fresh revision are a fresh base"
    );
    assert!(
        bases[..seat_count]
            .iter()
            .all(|(_, base)| *base == old_occurrence)
    );
    assert!(
        bases[seat_count..]
            .iter()
            .all(|(_, base)| *base == new_occurrence)
    );
    let final_raw = mg_state
        .meta_consensus_raw
        .lock()
        .expect("test lock")
        .clone()
        .expect("maintenance published metadata");
    let final_fields: BTreeMap<String, serde_json::Value> =
        serde_json::from_slice(&final_raw).expect("updated metadata parses");
    assert_eq!(
        final_fields.get(WELCOME_MESSAGE_META_FIELD_KEY),
        Some(&serde_json::Value::String("Hello again".to_owned()))
    );
}

#[tokio::test]
async fn already_adopted_update_needs_no_live_mg() {
    let database = MemDatabase::new().into_database();
    let (payments, _) = TestPayments::new();
    let mg_state = Arc::new(MgState::default());
    let reader = TestConsensusReader::new(mg_state.clone());
    let client = open_client_with_reader(
        database,
        payments,
        mg_state.clone(),
        MgConfig::given_away(),
        reader,
    )
    .await;
    client
        .buy_and_form_at(intent(), locators(), options())
        .await
        .unwrap();

    // Formation's threshold directory submissions would otherwise remain the
    // fake reader's preferred consensus source and intentionally overwrite a
    // directly installed maintenance snapshot below. The real federation has
    // already folded that directory into its whole metadata object at this
    // point, so clear only the fake's historical proposal log.
    mg_state.meta_submissions.lock().expect("test lock").clear();

    let mut fields: BTreeMap<String, serde_json::Value> = serde_json::from_slice(
        mg_state
            .meta_consensus_raw
            .lock()
            .expect("test lock")
            .as_deref()
            .expect("formed metadata"),
    )
    .unwrap();
    fields.insert(
        FEDERATION_NAME_META_FIELD_KEY.to_owned(),
        serde_json::Value::String("Already Adopted".to_owned()),
    );
    *mg_state.meta_consensus_raw.lock().expect("test lock") =
        Some(serde_json::to_vec(&fields).unwrap());
    *mg_state.offline_indices.lock().expect("test lock") =
        (0..usize::from(MIN_FEDERATION_SIZE)).collect();
    mg_state.connect_calls.store(0, Ordering::SeqCst);

    client
        .update_federation_metadata(
            FederationMetadataUpdate::name("Already Adopted").unwrap(),
            maintenance_options(),
        )
        .await
        .unwrap();
    assert_eq!(mg_state.connect_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn complete_consensus_object_cap_is_inclusive_and_checked_before_fanout() {
    fn name_object(total_bytes: usize, name: &str) -> Vec<u8> {
        let empty = serde_json::to_vec(&BTreeMap::from([
            (
                FEDERATION_NAME_META_FIELD_KEY.to_owned(),
                serde_json::Value::String(name.to_owned()),
            ),
            (
                "padding".to_owned(),
                serde_json::Value::String(String::new()),
            ),
        ]))
        .expect("fixture serializes");
        assert!(total_bytes >= empty.len());
        serde_json::to_vec(&BTreeMap::from([
            (
                FEDERATION_NAME_META_FIELD_KEY.to_owned(),
                serde_json::Value::String(name.to_owned()),
            ),
            (
                "padding".to_owned(),
                serde_json::Value::String("a".repeat(total_bytes - empty.len())),
            ),
        ]))
        .expect("fixture serializes")
    }

    let database = MemDatabase::new().into_database();
    let (payments, _) = TestPayments::new();
    let mg_state = Arc::new(MgState::default());
    let reader = TestConsensusReader::new(mg_state.clone());
    let client = open_client_with_reader(
        database,
        payments,
        mg_state.clone(),
        MgConfig::given_away(),
        reader,
    )
    .await;
    client
        .buy_and_form_at(intent(), locators(), options())
        .await
        .unwrap();

    // Leave the fake reader no historical threshold proposal to prefer over
    // the complete raw consensus object installed below.
    mg_state.meta_submissions.lock().expect("test lock").clear();
    mg_state
        .meta_request_bases
        .lock()
        .expect("test lock")
        .clear();
    mg_state.connect_calls.store(0, Ordering::SeqCst);

    let at_cap = name_object(FEDERATION_METADATA_OBJECT_MAX_BYTES, "At Object Cap");
    assert_eq!(at_cap.len(), FEDERATION_METADATA_OBJECT_MAX_BYTES);
    *mg_state.meta_consensus_raw.lock().expect("test lock") = Some(at_cap);
    client
        .update_federation_metadata(
            FederationMetadataUpdate::name("At Object Cap").unwrap(),
            maintenance_options(),
        )
        .await
        .expect("the inclusive complete-object cap is readable");
    assert_eq!(mg_state.connect_calls.load(Ordering::SeqCst), 0);

    let oversized = name_object(FEDERATION_METADATA_OBJECT_MAX_BYTES + 1, "Oversized Object");
    assert_eq!(oversized.len(), FEDERATION_METADATA_OBJECT_MAX_BYTES + 1);
    *mg_state.meta_consensus_raw.lock().expect("test lock") = Some(oversized);
    let error = client
        .update_federation_metadata(
            FederationMetadataUpdate::name("Oversized Object").unwrap(),
            maintenance_options(),
        )
        .await
        .expect_err("an oversized complete object is terminal before fanout");
    assert!(matches!(
        error,
        FaError::MaintenanceConsensusTooLarge {
            actual_bytes,
            max_bytes: FEDERATION_METADATA_OBJECT_MAX_BYTES,
        } if actual_bytes == FEDERATION_METADATA_OBJECT_MAX_BYTES + 1
    ));
    assert_eq!(mg_state.connect_calls.load(Ordering::SeqCst), 0);
    assert!(
        mg_state
            .meta_request_bases
            .lock()
            .expect("test lock")
            .is_empty()
    );
}

#[tokio::test]
async fn malformed_consensus_object_is_a_typed_terminal_error_before_fanout() {
    let database = MemDatabase::new().into_database();
    let (payments, _) = TestPayments::new();
    let mg_state = Arc::new(MgState::default());
    let reader = TestConsensusReader::new(mg_state.clone());
    let client = open_client_with_reader(
        database,
        payments,
        mg_state.clone(),
        MgConfig::given_away(),
        reader,
    )
    .await;
    client
        .buy_and_form_at(intent(), locators(), options())
        .await
        .unwrap();
    mg_state.meta_submissions.lock().expect("test lock").clear();
    mg_state
        .meta_request_bases
        .lock()
        .expect("test lock")
        .clear();
    *mg_state.meta_consensus_raw.lock().expect("test lock") = Some(b"not-json".to_vec());
    mg_state.connect_calls.store(0, Ordering::SeqCst);

    assert!(matches!(
        client
            .update_federation_metadata(
                FederationMetadataUpdate::name("Cannot Parse").unwrap(),
                maintenance_options(),
            )
            .await,
        Err(FaError::MaintenanceConsensusInvalid { .. })
    ));
    assert_eq!(mg_state.connect_calls.load(Ordering::SeqCst), 0);
    assert!(
        mg_state
            .meta_request_bases
            .lock()
            .expect("test lock")
            .is_empty()
    );
}

#[tokio::test]
async fn formation_refuses_an_oversized_consensus_object_before_metadata_fanout() {
    let database = MemDatabase::new().into_database();
    let (payments, _) = TestPayments::new();
    let mg_state = Arc::new(MgState::default());
    let reader = TestConsensusReader::new(mg_state.clone());
    reader.force_value(&"x".repeat(FEDERATION_METADATA_OBJECT_MAX_BYTES));
    let client = open_client_with_reader(
        database,
        payments,
        mg_state.clone(),
        MgConfig::given_away(),
        reader,
    )
    .await;

    let error = client
        .buy_and_form_at(intent(), locators(), options())
        .await
        .expect_err("formation cannot amplify an oversized live meta object");
    assert!(matches!(
        error,
        FaError::InvalidManifoldGuardians(message)
            if message.contains("formation permits at most 1048576 bytes")
    ));
    assert!(
        mg_state
            .meta_request_bases
            .lock()
            .expect("test lock")
            .is_empty(),
        "no seat-binding submission is signed or fanned out"
    );
}

#[tokio::test]
async fn threshold_live_partial_wave_succeeds_and_late_timeouts_do_not_mask_adoption() {
    let database = MemDatabase::new().into_database();
    let (payments, _) = TestPayments::new();
    let mg_state = Arc::new(MgState::default());
    let reader = TestConsensusReader::new(mg_state.clone());
    let client = open_client_with_reader(
        database,
        payments,
        mg_state.clone(),
        MgConfig::given_away(),
        reader,
    )
    .await;
    client
        .buy_and_form_at(intent(), locators(), options())
        .await
        .unwrap();

    mg_state.meta_submissions.lock().expect("test lock").clear();
    mg_state
        .meta_request_bases
        .lock()
        .expect("test lock")
        .clear();
    *mg_state.hang_meta_indices.lock().expect("test lock") = [0, 1].into_iter().collect();

    client
        .update_federation_metadata(
            FederationMetadataUpdate::welcome_message("Threshold live").unwrap(),
            maintenance_options(),
        )
        .await
        .unwrap();
    assert_eq!(
        mg_state.meta_submissions.lock().expect("test lock").len(),
        test_federation_seats().consensus_threshold() as usize
    );
}

#[tokio::test(start_paused = true)]
async fn below_threshold_availability_fails_within_the_driver_deadline() {
    let database = MemDatabase::new().into_database();
    let (payments, _) = TestPayments::new();
    let mg_state = Arc::new(MgState::default());
    let reader = TestConsensusReader::new(mg_state.clone());
    let client = open_client_with_reader(
        database,
        payments,
        mg_state.clone(),
        MgConfig::given_away(),
        reader,
    )
    .await;
    client
        .buy_and_form_at(intent(), locators(), options())
        .await
        .unwrap();

    mg_state.meta_submissions.lock().expect("test lock").clear();
    mg_state
        .meta_request_bases
        .lock()
        .expect("test lock")
        .clear();
    // A lost response is transport-ambiguous: the mock hangs, so the row
    // surfaces as the driver's request timeout, never as a serialized wire
    // refusal (transport failure lives outside `ManifoldGuardianError`).
    *mg_state.hang_meta_indices.lock().expect("test lock") = [0, 1, 2].into_iter().collect();
    let bounded = MaintenanceRunOptions::new(MaintenanceRunOptionsConfig {
        poll_interval: Duration::from_millis(1),
        run_timeout: Duration::from_millis(50),
        request_timeout: Duration::from_millis(10),
    })
    .unwrap();

    mg_state
        .meta_request_wave_size
        .store(usize::from(MIN_FEDERATION_SIZE), Ordering::SeqCst);
    let first_wave = mg_state.meta_request_wave_complete.notified();
    let operation = tokio::spawn({
        let client = client.clone();
        async move {
            client
                .update_federation_metadata(
                    FederationMetadataUpdate::name("Cannot Converge").unwrap(),
                    bounded,
                )
                .await
        }
    });
    // Keep the driver's monotonic clock at its start instant until the first
    // incomplete wave is definitely in flight. Then advance directly to the
    // configured deadline rather than relying on executor scheduling.
    let mut operation = operation;
    tokio::select! {
        () = first_wave => {}
        result = &mut operation => panic!("maintenance task ended before the first wave: {result:?}"),
    }
    tokio::time::advance(Duration::from_millis(50)).await;
    let error = operation
        .await
        .expect("maintenance task joins")
        .expect_err("four reachable guardians cannot meet a five-seat threshold");
    let FaError::MaintenanceConvergence {
        unresolved,
        guardian_errors,
        consensus_error,
    } = error
    else {
        panic!("below-threshold maintenance returned {error:?}")
    };
    assert_eq!(unresolved, vec![0, 1, 2]);
    // Which per-seat operation the deadline cuts (reconnecting, signing, or
    // submitting) is incidental phase; the property is that every diagnosis
    // is the driver's own sanitized operation label, never remote or endpoint
    // detail.
    assert_eq!(
        guardian_errors
            .iter()
            .map(|(row, _)| *row)
            .collect::<Vec<_>>(),
        vec![0, 1, 2],
    );
    for (row, message) in &guardian_errors {
        assert!(
            message == "submitting SetMetaField proposal"
                || message == "signing SetMetaField request"
                || message == "reconnecting to formed Manifold Guardian",
            "transport-ambiguous row {row} surfaced a non-driver diagnosis: {message:?}"
        );
    }
    // The deadline may end the run after any phase. Whether it instead lands
    // in a sleep (leaving no consensus failure) is incidental. Either way the
    // record must stay the driver's own operation label — never reader or
    // endpoint detail.
    match consensus_error.as_deref() {
        None | Some("reading federation consensus metadata") => {}
        other => panic!("unexpected consensus diagnostic: {other:?}"),
    }
}

#[tokio::test]
async fn transient_transport_retries_only_the_unresolved_rows_until_readback_succeeds() {
    let database = MemDatabase::new().into_database();
    let (payments, _) = TestPayments::new();
    let mg_state = Arc::new(MgState::default());
    let reader = TestConsensusReader::new(mg_state.clone());
    let client = open_client_with_reader(
        database,
        payments,
        mg_state.clone(),
        MgConfig::given_away(),
        reader,
    )
    .await;
    client
        .buy_and_form_at(intent(), locators(), options())
        .await
        .unwrap();

    mg_state.meta_submissions.lock().expect("test lock").clear();
    mg_state
        .meta_request_bases
        .lock()
        .expect("test lock")
        .clear();
    // Injected lost responses: the rows hang until the request timeout, the
    // transport-ambiguous shape (a wire `ManifoldGuardianError` cannot carry it).
    *mg_state.hang_meta_indices.lock().expect("test lock") = [0, 1, 2].into_iter().collect();

    let operation = tokio::spawn({
        let client = client.clone();
        async move {
            client
                .update_federation_metadata(
                    FederationMetadataUpdate::name("Recovered transport").unwrap(),
                    maintenance_options(),
                )
                .await
        }
    });
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if mg_state.meta_request_bases.lock().expect("test lock").len()
                >= usize::from(MIN_FEDERATION_SIZE)
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the first below-threshold wave completes");
    mg_state
        .hang_meta_indices
        .lock()
        .expect("test lock")
        .remove(&0);

    operation
        .await
        .expect("maintenance task joins")
        .expect("one recovered row lets threshold readback converge");

    let attempts = mg_state.meta_request_bases.lock().expect("test lock");
    let attempts_for = |index| attempts.iter().filter(|(row, _)| *row == index).count();
    assert!(attempts_for(0) >= 2, "the recovered row is retried");
    for acknowledged in 3..7 {
        assert_eq!(
            attempts_for(acknowledged),
            1,
            "an acknowledged sibling is retained for the unchanged base"
        );
    }
    let adopted = mg_state
        .meta_consensus_raw
        .lock()
        .expect("test lock")
        .clone()
        .expect("threshold readback contains the adopted object");
    let fields: BTreeMap<String, serde_json::Value> =
        serde_json::from_slice(&adopted).expect("adopted metadata parses");
    assert_eq!(
        fields.get(FEDERATION_NAME_META_FIELD_KEY),
        Some(&serde_json::Value::String("Recovered transport".to_owned()))
    );
}

#[tokio::test]
async fn terminal_guardian_refusal_is_typed_and_not_retried() {
    let database = MemDatabase::new().into_database();
    let (payments, _) = TestPayments::new();
    let mg_state = Arc::new(MgState::default());
    let client = open_client(database, payments, mg_state.clone(), MgConfig::given_away()).await;
    client
        .buy_and_form_at(intent(), locators(), options())
        .await
        .unwrap();

    mg_state.meta_submissions.lock().expect("test lock").clear();
    mg_state
        .meta_request_bases
        .lock()
        .expect("test lock")
        .clear();
    *mg_state.offline_indices.lock().expect("test lock") = [0, 1].into_iter().collect();
    mg_state
        .meta_terminal_errors
        .lock()
        .expect("test lock")
        .insert(2, ManifoldGuardianError::MetaValueInvalid);

    let error = client
        .update_federation_metadata(
            FederationMetadataUpdate::name("Terminal Refusal").unwrap(),
            maintenance_options(),
        )
        .await
        .expect_err("four accepting seats cannot hide the fifth seat's terminal refusal");
    assert!(matches!(
        error,
        FaError::MaintenanceRejected {
            index: 2,
            reason: ManifoldGuardianError::MetaValueInvalid,
        }
    ));
    assert_eq!(
        mg_state
            .meta_request_bases
            .lock()
            .expect("test lock")
            .iter()
            .filter(|(index, _)| *index == 2)
            .count(),
        1,
        "a terminal refusal is never polled as transient unavailability"
    );
}

#[tokio::test]
async fn pinned_target_conflict_stops_same_base_retries_and_is_distinguishable() {
    let database = MemDatabase::new().into_database();
    let (payments, _) = TestPayments::new();
    let mg_state = Arc::new(MgState::default());
    let reader = TestConsensusReader::new(mg_state.clone());
    let client = open_client_with_reader(
        database,
        payments,
        mg_state.clone(),
        MgConfig::given_away(),
        reader,
    )
    .await;
    client
        .buy_and_form_at(intent(), locators(), options())
        .await
        .unwrap();

    mg_state.meta_submissions.lock().expect("test lock").clear();
    mg_state
        .meta_request_bases
        .lock()
        .expect("test lock")
        .clear();
    // Three guardians pinned the active base to a different admitted target:
    // a below-threshold wedge that same-base retries cannot clear.
    {
        let mut failures = mg_state.meta_terminal_errors.lock().expect("test lock");
        for index in 0..3 {
            failures.insert(index, ManifoldGuardianError::MetaTargetConflict);
        }
    }
    let bounded = MaintenanceRunOptions::new(MaintenanceRunOptionsConfig {
        poll_interval: Duration::from_millis(1),
        run_timeout: Duration::from_millis(50),
        request_timeout: Duration::from_millis(10),
    })
    .unwrap();

    let error = client
        .update_federation_metadata(
            FederationMetadataUpdate::name("Pinned Conflict").unwrap(),
            bounded,
        )
        .await
        .expect_err("four unpinned guardians cannot meet a five-seat threshold");
    let FaError::MaintenanceConvergence {
        unresolved,
        guardian_errors,
        consensus_error,
    } = error
    else {
        panic!("pinned-conflict maintenance returned {error:?}")
    };
    assert_eq!(unresolved, vec![0, 1, 2]);
    let pinned_diagnosis = "guardian already admitted a different metadata target for this \
                            consensus base; retrying cannot help until the conflicting write \
                            converges or the guardian's operator restarts it";
    assert_eq!(
        guardian_errors,
        vec![
            (0, pinned_diagnosis.to_owned()),
            (1, pinned_diagnosis.to_owned()),
            (2, pinned_diagnosis.to_owned()),
        ],
        "the pinned-conflict diagnosis is distinguishable from a stale base or lost response"
    );
    // The pinned conflicts are terminal after their first attempt, but the
    // whole-driver deadline can still expire while the final consensus
    // readback runs. That incidental phase records the same sanitized
    // diagnostic as the below-threshold deadline witness; it does not make a
    // pinned seat retryable or change the pinned-target result asserted below.
    match consensus_error.as_deref() {
        None | Some("reading federation consensus metadata") => {}
        other => panic!("unexpected consensus diagnostic: {other:?}"),
    }

    let attempts = mg_state.meta_request_bases.lock().expect("test lock");
    let attempts_for = |index| attempts.iter().filter(|(row, _)| *row == index).count();
    for pinned in 0..3 {
        assert_eq!(
            attempts_for(pinned),
            1,
            "a pinned seat receives no further same-base submissions"
        );
    }
    for acknowledged in 3..7 {
        assert_eq!(
            attempts_for(acknowledged),
            1,
            "acknowledged siblings are retained for the unchanged base"
        );
    }
}

#[tokio::test]
async fn transient_read_failure_is_retried_without_changing_the_mutation() {
    let database = MemDatabase::new().into_database();
    let (payments, _) = TestPayments::new();
    let mg_state = Arc::new(MgState::default());
    let reader = TestConsensusReader::new(mg_state.clone());
    let client = open_client_with_reader(
        database,
        payments,
        mg_state.clone(),
        MgConfig::given_away(),
        reader.clone(),
    )
    .await;
    client
        .buy_and_form_at(intent(), locators(), options())
        .await
        .unwrap();
    mg_state.meta_submissions.lock().expect("test lock").clear();
    reader.fail_next(1);

    client
        .update_federation_metadata(
            FederationMetadataUpdate::icon_url("https://example.com/new.png").unwrap(),
            maintenance_options(),
        )
        .await
        .unwrap();
    assert!(
        mg_state
            .meta_submissions
            .lock()
            .expect("test lock")
            .values()
            .all(|(key, value)| key == FEDERATION_ICON_URL_META_FIELD_KEY
                && value == "https://example.com/new.png")
    );
}

#[tokio::test]
async fn cancelled_partial_wave_reopens_and_replays_without_losing_unrelated_metadata() {
    let database = MemDatabase::new().into_database();
    let (payments, _) = TestPayments::new();
    let mg_state = Arc::new(MgState::default());
    let reader = TestConsensusReader::new(mg_state.clone());
    let client = open_client_with_reader(
        database.clone(),
        payments.clone(),
        mg_state.clone(),
        MgConfig::given_away(),
        reader.clone(),
    )
    .await;
    client
        .buy_and_form_at(intent(), locators(), options())
        .await
        .unwrap();

    mg_state.meta_submissions.lock().expect("test lock").clear();
    mg_state
        .meta_request_bases
        .lock()
        .expect("test lock")
        .clear();
    let mut initial_fields: BTreeMap<String, serde_json::Value> = serde_json::from_slice(
        mg_state
            .meta_consensus_raw
            .lock()
            .expect("test lock")
            .as_deref()
            .expect("formed metadata"),
    )
    .unwrap();
    initial_fields.insert(
        "unrelated".to_owned(),
        serde_json::Value::String("preserved".to_owned()),
    );
    *mg_state.meta_consensus_raw.lock().expect("test lock") =
        Some(serde_json::to_vec(&initial_fields).unwrap());

    // Leave exactly a threshold live. The operation reaches a real submitted
    // partial wave, then remains pending on the two deliberately hung siblings
    // until the test cancels it before consensus readback.
    *mg_state.hang_meta_indices.lock().expect("test lock") = [0, 1].into_iter().collect();
    let operation_client = client.clone();
    let operation = tokio::spawn(async move {
        operation_client
            .update_federation_metadata(
                FederationMetadataUpdate::welcome_message("Survives cancellation").unwrap(),
                long_maintenance_options(),
            )
            .await
    });
    let threshold = test_federation_seats().consensus_threshold() as usize;
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if mg_state.meta_submissions.lock().expect("test lock").len() >= threshold {
                break;
            }
            mg_state.meta_submission_changed.notified().await;
        }
    })
    .await
    .expect("partial metadata wave reached the consensus threshold");
    operation.abort();
    assert!(operation.await.unwrap_err().is_cancelled());
    let connects_before_replay = mg_state.connect_calls.load(Ordering::SeqCst);

    // Reopen from the same durable namespace. The replay reads the
    // threshold-adopted value and needs no live MG connection.
    drop(client);
    *mg_state.offline_indices.lock().expect("test lock") =
        (0..usize::from(MIN_FEDERATION_SIZE)).collect();
    let reopened = open_client_with_reader(
        database,
        payments,
        mg_state.clone(),
        MgConfig::given_away(),
        reader,
    )
    .await;
    // Maintenance waits for the reopened federation to reconcile.
    reopened.drive().await.unwrap();
    reopened
        .update_federation_metadata(
            FederationMetadataUpdate::welcome_message("Survives cancellation").unwrap(),
            maintenance_options(),
        )
        .await
        .unwrap();
    assert_eq!(
        mg_state.connect_calls.load(Ordering::SeqCst),
        connects_before_replay
    );

    let final_fields: BTreeMap<String, serde_json::Value> = serde_json::from_slice(
        mg_state
            .meta_consensus_raw
            .lock()
            .expect("test lock")
            .as_deref()
            .expect("replayed metadata"),
    )
    .unwrap();
    assert_eq!(
        final_fields.get("unrelated"),
        Some(&serde_json::Value::String("preserved".to_owned()))
    );
    assert_eq!(
        final_fields.get(WELCOME_MESSAGE_META_FIELD_KEY),
        Some(&serde_json::Value::String(
            "Survives cancellation".to_owned()
        ))
    );
}
