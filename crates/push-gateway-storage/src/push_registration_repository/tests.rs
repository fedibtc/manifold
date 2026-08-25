use sqlx::{AnyPool, Row, any::AnyPoolOptions};

use super::*;
use fedi_decentralized_push_gateway_types::{
    DeviceInstallationId, FcmRegistrationToken, Platform, RecipientId, RegisterInstallationRequest,
};

#[tokio::test]
async fn replacing_installation_token_leaves_one_registration() {
    let pool = test_pool().await;
    let repository = PushRegistrationRepository::new(pool.clone());
    let recipient_id = RecipientId("recipient".to_owned());
    let mut request = RegisterInstallationRequest {
        installation_id: DeviceInstallationId("installation".to_owned()),
        fcm_token: FcmRegistrationToken("token-1".to_owned()),
        platform: Some(Platform("android".to_owned())),
    };

    repository
        .admit_installation(&recipient_id, &request, eligibility(), unlimited_limits())
        .await
        .expect("insert first registration");
    request.fcm_token = FcmRegistrationToken("token-2".to_owned());
    repository
        .admit_installation(&recipient_id, &request, eligibility(), unlimited_limits())
        .await
        .expect("replace registration token");

    let row = sqlx::query(
        "SELECT COUNT(*) AS count, MAX(fcm_token) AS fcm_token
             FROM push_registrations
             WHERE recipient_id = $1 AND installation_id = $2",
    )
    .bind(&recipient_id.0)
    .bind(&request.installation_id.0)
    .fetch_one(&pool)
    .await
    .expect("fetch registration count");

    let count: i64 = row.get("count");
    let token: String = row.get("fcm_token");
    assert_eq!(count, 1);
    assert_eq!(token, "token-2");
}

#[tokio::test]
async fn recipient_lookup_omits_disabled_and_delete_removes_registration() {
    let pool = test_pool().await;
    let repository = PushRegistrationRepository::new(pool);
    let recipient_id = RecipientId("recipient".to_owned());
    let request = RegisterInstallationRequest {
        installation_id: DeviceInstallationId("installation".to_owned()),
        fcm_token: FcmRegistrationToken("token-1".to_owned()),
        platform: Some(Platform("ios".to_owned())),
    };

    repository
        .admit_installation(&recipient_id, &request, eligibility(), unlimited_limits())
        .await
        .expect("insert registration");
    let registrations = repository
        .list_for_recipient(&recipient_id, eligibility())
        .await
        .expect("list active registrations");
    assert_eq!(registrations.len(), 1);
    assert_eq!(registrations[0].last_seen_at, registrations[0].created_at);

    assert!(
        repository
            .disable_installation(
                &recipient_id,
                &request.installation_id,
                Some("invalid_token")
            )
            .await
            .expect("disable registration")
    );
    assert!(
        repository
            .list_for_recipient(&recipient_id, eligibility())
            .await
            .expect("list after disable")
            .is_empty()
    );

    assert!(
        repository
            .delete_installation(&recipient_id, &request.installation_id)
            .await
            .expect("delete registration")
    );
}

#[tokio::test]
async fn same_installation_id_can_exist_for_different_recipients() {
    let pool = test_pool().await;
    let repository = PushRegistrationRepository::new(pool);
    let old_recipient_id = RecipientId("old-recipient".to_owned());
    let new_recipient_id = RecipientId("new-recipient".to_owned());
    let mut request = RegisterInstallationRequest {
        installation_id: DeviceInstallationId("installation".to_owned()),
        fcm_token: FcmRegistrationToken("old-token".to_owned()),
        platform: None,
    };

    repository
        .admit_installation(
            &old_recipient_id,
            &request,
            eligibility(),
            unlimited_limits(),
        )
        .await
        .expect("insert old recipient registration");
    request.fcm_token = FcmRegistrationToken("new-token".to_owned());
    repository
        .admit_installation(
            &new_recipient_id,
            &request,
            eligibility(),
            unlimited_limits(),
        )
        .await
        .expect("insert same installation id for new recipient");

    let old_registrations = repository
        .list_for_recipient(&old_recipient_id, eligibility())
        .await
        .expect("list old recipient");
    assert_eq!(old_registrations.len(), 1);
    assert_eq!(old_registrations[0].fcm_token.0, "old-token");
    let registrations = repository
        .list_for_recipient(&new_recipient_id, eligibility())
        .await
        .expect("list new recipient");
    assert_eq!(registrations.len(), 1);
    assert_eq!(registrations[0].fcm_token.0, "new-token");
}

#[tokio::test]
async fn fcm_token_reassigns_same_installation_across_recipients_without_logout_cleanup() {
    let pool = test_pool().await;
    let repository = PushRegistrationRepository::new(pool);
    let old_recipient_id = RecipientId("old-recipient".to_owned());
    let new_recipient_id = RecipientId("new-recipient".to_owned());
    let request = RegisterInstallationRequest {
        installation_id: DeviceInstallationId("installation".to_owned()),
        fcm_token: FcmRegistrationToken("token".to_owned()),
        platform: None,
    };

    repository
        .admit_installation(
            &old_recipient_id,
            &request,
            eligibility(),
            unlimited_limits(),
        )
        .await
        .expect("insert old recipient registration");
    let outcome = repository
        .admit_installation(
            &new_recipient_id,
            &request,
            eligibility(),
            unlimited_limits(),
        )
        .await
        .expect("move installation route to new recipient");
    assert_eq!(outcome, RegistrationAdmissionOutcome::Registered);

    assert!(
        repository
            .list_for_recipient(&old_recipient_id, eligibility())
            .await
            .expect("list old recipient")
            .is_empty()
    );
    let new_registrations = repository
        .list_for_recipient(&new_recipient_id, eligibility())
        .await
        .expect("list new recipient");
    assert_eq!(new_registrations.len(), 1);
    assert_eq!(new_registrations[0].installation_id.0, "installation");
    assert_eq!(new_registrations[0].fcm_token.0, "token");
}

#[tokio::test]
async fn stale_gc_retains_same_installation_handoff_and_rejects_a_different_installation() {
    let pool = test_pool().await;
    let repository = PushRegistrationRepository::new(pool.clone());
    let old_recipient = RecipientId("old-recipient".to_owned());
    let new_recipient = RecipientId("new-recipient".to_owned());
    let request = RegisterInstallationRequest {
        installation_id: DeviceInstallationId("installation".to_owned()),
        fcm_token: FcmRegistrationToken("token".to_owned()),
        platform: None,
    };

    assert_eq!(
        repository
            .admit_installation(&old_recipient, &request, eligibility(), unlimited_limits(),)
            .await
            .unwrap(),
        RegistrationAdmissionOutcome::Registered
    );
    sqlx::query("UPDATE push_registrations SET last_seen_at = 1")
        .execute(&pool)
        .await
        .unwrap();

    let gc_request = RegisterInstallationRequest {
        installation_id: DeviceInstallationId("gc-installation".to_owned()),
        fcm_token: FcmRegistrationToken("gc-token".to_owned()),
        platform: None,
    };
    assert_eq!(
        repository
            .admit_installation(
                &new_recipient,
                &gc_request,
                RegistrationEligibility {
                    cutoff_timestamp: 2,
                },
                unlimited_limits(),
            )
            .await
            .unwrap(),
        RegistrationAdmissionOutcome::Registered
    );

    let hijack_request = RegisterInstallationRequest {
        installation_id: DeviceInstallationId("different-installation".to_owned()),
        ..request.clone()
    };
    assert_eq!(
        repository
            .admit_installation(
                &new_recipient,
                &hijack_request,
                RegistrationEligibility {
                    cutoff_timestamp: 2,
                },
                unlimited_limits(),
            )
            .await
            .unwrap(),
        RegistrationAdmissionOutcome::TokenBoundToDifferentInstallation
    );
    let registrations: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM push_registrations")
        .fetch_one(&pool)
        .await
        .unwrap();
    let token_owners: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM push_registration_token_owners")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(registrations, 1, "bounded GC reclaimed only the stale row");
    assert_eq!(token_owners, 2, "handoff identity survives stale-row GC");

    assert_eq!(
        repository
            .admit_installation(
                &new_recipient,
                &request,
                RegistrationEligibility {
                    cutoff_timestamp: 2,
                },
                unlimited_limits(),
            )
            .await
            .unwrap(),
        RegistrationAdmissionOutcome::Registered
    );
    let owner_recipient: String = sqlx::query_scalar(
        "SELECT recipient_id FROM push_registration_token_owners WHERE fcm_token = $1",
    )
    .bind(&request.fcm_token.0)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(owner_recipient, new_recipient.0);
}

#[tokio::test]
async fn stale_orphan_reclamation_recovers_saturated_physical_capacity() {
    let pool = test_pool().await;
    let repository = PushRegistrationRepository::new(pool.clone());
    let recipient = RecipientId("recipient".to_owned());
    let limits = RegistrationAdmissionLimits {
        max_active_per_recipient: 0,
        max_active_global: 0,
        max_total_rows: 2,
        reclamation_batch_size: 1,
    };
    let stale = RegisterInstallationRequest {
        installation_id: DeviceInstallationId("stale-installation".to_owned()),
        fcm_token: FcmRegistrationToken("stale-token".to_owned()),
        platform: None,
    };
    assert_eq!(
        repository
            .admit_installation(&recipient, &stale, eligibility(), limits)
            .await
            .unwrap(),
        RegistrationAdmissionOutcome::Registered
    );
    sqlx::query("UPDATE push_registrations SET last_seen_at = 1")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE push_registration_token_owners SET updated_at = 1")
        .execute(&pool)
        .await
        .unwrap();

    let replacement = RegisterInstallationRequest {
        installation_id: DeviceInstallationId("replacement-installation".to_owned()),
        fcm_token: FcmRegistrationToken("replacement-token".to_owned()),
        platform: None,
    };
    assert_eq!(
        repository
            .admit_installation(
                &recipient,
                &replacement,
                RegistrationEligibility {
                    cutoff_timestamp: 2,
                },
                limits,
            )
            .await
            .unwrap(),
        RegistrationAdmissionOutcome::Registered
    );
    let metrics = repository
        .row_metrics(RegistrationEligibility {
            cutoff_timestamp: 2,
        })
        .await
        .unwrap();
    assert_eq!(metrics.total, 2);
    assert_eq!(metrics.registrations, 1);
    assert_eq!(metrics.token_owners, 1);
    assert_eq!(metrics.orphaned_token_owners, 0);
}

#[tokio::test]
async fn cross_recipient_handoff_respects_new_recipient_active_capacity() {
    let pool = test_pool().await;
    let repository = PushRegistrationRepository::new(pool);
    let old_recipient = RecipientId("old-recipient".to_owned());
    let new_recipient = RecipientId("new-recipient".to_owned());
    let limits = RegistrationAdmissionLimits {
        max_active_per_recipient: 1,
        max_active_global: 2,
        max_total_rows: 4,
        reclamation_batch_size: 10,
    };
    let moving = RegisterInstallationRequest {
        installation_id: DeviceInstallationId("moving-installation".to_owned()),
        fcm_token: FcmRegistrationToken("moving-token".to_owned()),
        platform: None,
    };
    let existing = RegisterInstallationRequest {
        installation_id: DeviceInstallationId("existing-installation".to_owned()),
        fcm_token: FcmRegistrationToken("existing-token".to_owned()),
        platform: None,
    };

    assert_eq!(
        repository
            .admit_installation(&old_recipient, &moving, eligibility(), limits)
            .await
            .unwrap(),
        RegistrationAdmissionOutcome::Registered
    );
    assert_eq!(
        repository
            .admit_installation(&new_recipient, &existing, eligibility(), limits)
            .await
            .unwrap(),
        RegistrationAdmissionOutcome::Registered
    );
    assert_eq!(
        repository
            .admit_installation(&new_recipient, &moving, eligibility(), limits)
            .await
            .unwrap(),
        RegistrationAdmissionOutcome::RecipientCapacityExceeded
    );
}

#[tokio::test]
async fn same_recipient_token_move_is_count_neutral_at_active_and_physical_caps() {
    let pool = test_pool().await;
    let repository = PushRegistrationRepository::new(pool);
    let recipient = RecipientId("recipient".to_owned());
    let mut request = RegisterInstallationRequest {
        installation_id: DeviceInstallationId("old-installation".to_owned()),
        fcm_token: FcmRegistrationToken("token".to_owned()),
        platform: None,
    };
    let limits = RegistrationAdmissionLimits {
        max_active_per_recipient: 1,
        max_active_global: 1,
        max_total_rows: 2,
        reclamation_batch_size: 10,
    };
    assert_eq!(
        repository
            .admit_installation(&recipient, &request, eligibility(), limits)
            .await
            .unwrap(),
        RegistrationAdmissionOutcome::Registered
    );
    request.installation_id = DeviceInstallationId("new-installation".to_owned());
    assert_eq!(
        repository
            .admit_installation(&recipient, &request, eligibility(), limits)
            .await
            .unwrap(),
        RegistrationAdmissionOutcome::Registered
    );
    let registrations = repository
        .list_for_recipient(&recipient, eligibility())
        .await
        .unwrap();
    assert_eq!(registrations.len(), 1);
    assert_eq!(registrations[0].installation_id.0, "new-installation");
}

#[tokio::test]
async fn count_increasing_registration_is_refused_at_exact_cap() {
    let pool = test_pool().await;
    let repository = PushRegistrationRepository::new(pool);
    let recipient = RecipientId("recipient".to_owned());
    let limits = RegistrationAdmissionLimits {
        max_active_per_recipient: 1,
        max_active_global: 1,
        max_total_rows: 2,
        reclamation_batch_size: 10,
    };
    for (installation, token, expected) in [
        ("first", "token-1", RegistrationAdmissionOutcome::Registered),
        (
            "second",
            "token-2",
            RegistrationAdmissionOutcome::RecipientCapacityExceeded,
        ),
    ] {
        let request = RegisterInstallationRequest {
            installation_id: DeviceInstallationId(installation.to_owned()),
            fcm_token: FcmRegistrationToken(token.to_owned()),
            platform: None,
        };
        assert_eq!(
            repository
                .admit_installation(&recipient, &request, eligibility(), limits)
                .await
                .unwrap(),
            expected
        );
    }
}

fn eligibility() -> RegistrationEligibility {
    RegistrationEligibility {
        cutoff_timestamp: 0,
    }
}

fn unlimited_limits() -> RegistrationAdmissionLimits {
    RegistrationAdmissionLimits {
        max_active_per_recipient: 0,
        max_active_global: 0,
        max_total_rows: 0,
        reclamation_batch_size: 100,
    }
}

async fn test_pool() -> AnyPool {
    sqlx::any::install_default_drivers();
    let pool = AnyPoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("connect in-memory sqlite");
    sqlx::query(
        "CREATE TABLE push_registrations (
                recipient_id TEXT NOT NULL,
                installation_id TEXT NOT NULL,
                fcm_token TEXT NOT NULL,
                platform TEXT,
                created_at BIGINT NOT NULL,
                updated_at BIGINT NOT NULL,
                last_seen_at BIGINT NOT NULL,
                disabled_at BIGINT,
                disabled_reason TEXT,
                PRIMARY KEY (recipient_id, installation_id)
            )",
    )
    .execute(&pool)
    .await
    .expect("create push_registrations table");
    sqlx::query(
        "CREATE UNIQUE INDEX push_registrations_fcm_token_idx
             ON push_registrations (fcm_token)",
    )
    .execute(&pool)
    .await
    .expect("create fcm token index");
    sqlx::query(
        "CREATE TABLE push_registration_token_owners (
             fcm_token TEXT PRIMARY KEY,
             recipient_id TEXT NOT NULL,
             installation_id TEXT NOT NULL,
             updated_at BIGINT NOT NULL
         )",
    )
    .execute(&pool)
    .await
    .expect("create token owner table");
    sqlx::query(
        "CREATE TABLE push_gateway_admission_locks (
             resource TEXT PRIMARY KEY,
             updated_at BIGINT NOT NULL
         )",
    )
    .execute(&pool)
    .await
    .expect("create admission lock table");
    sqlx::query(
        "INSERT INTO push_gateway_admission_locks (resource, updated_at)
         VALUES ('registration', 0), ('hook', 0)",
    )
    .execute(&pool)
    .await
    .expect("seed admission locks");
    pool
}
