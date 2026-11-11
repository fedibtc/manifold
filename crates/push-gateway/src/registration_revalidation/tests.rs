use std::sync::Mutex;

use crate::{FcmRegistrationToken, Notification};
use sqlx::Executor;

use super::*;
use crate::{Database, ProviderFuture, PushProviderError};

/// Answers validate-only checks from a per-token script and records each call.
#[derive(Default)]
struct ScriptedProvider {
    calls: Mutex<Vec<String>>,
    /// Token whose check replaces the registration's token before answering,
    /// as the app re-registering mid-check would.
    replaced_mid_check: Option<(&'static str, Database)>,
}

impl PushProvider for ScriptedProvider {
    fn validate_registration<'a>(&'a self, token: &'a FcmRegistrationToken) -> ProviderFuture<'a> {
        Box::pin(async move {
            self.calls.lock().unwrap().push(token.0.clone());
            if let Some((replaced, database)) = &self.replaced_mid_check
                && token.0 == *replaced
            {
                database
                    .pool()
                    .execute("UPDATE push_registrations SET fcm_token = 'new-token' WHERE fcm_token = 'rotating'")
                    .await
                    .unwrap();
                return Err(PushProviderError::invalid_token("invalid_token"));
            }
            match token.0.as_str() {
                "uninstalled" => Err(PushProviderError::invalid_token("invalid_token")),
                flaky if flaky.starts_with("flaky") => {
                    Err(PushProviderError::unavailable("provider_transient"))
                }
                _ => Ok(()),
            }
        })
    }

    fn deliver<'a>(
        &'a self,
        _registration: &'a PushRegistration,
        _notification: &'a Notification,
    ) -> ProviderFuture<'a> {
        unreachable!("revalidation never delivers")
    }
}

async fn database_with(rows: &[(&str, &str, i64)]) -> (tempfile::TempDir, Database) {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let database_url = format!(
        "sqlite://{}?mode=rwc",
        tempdir.path().join("push.sqlite").display()
    );
    let database = Database::connect(&database_url)
        .await
        .expect("connect database");
    for (installation, token, validated_at) in rows {
        sqlx::query(
            "INSERT INTO push_registrations (
                recipient_id, installation_id, fcm_token, created_at, updated_at,
                last_seen_at, validated_at
             ) VALUES ('recipient', $1, $2, 1, 1, 1, $3)",
        )
        .bind(*installation)
        .bind(*token)
        .bind(*validated_at)
        .execute(database.pool())
        .await
        .expect("insert registration");
    }
    (tempdir, database)
}

async fn state_of(database: &Database, installation: &str) -> (String, i64, Option<String>) {
    let row = sqlx::query(
        "SELECT fcm_token, validated_at, disabled_reason FROM push_registrations
         WHERE installation_id = $1",
    )
    .bind(installation)
    .fetch_one(database.pool())
    .await
    .expect("registration row");
    use sqlx::Row;
    (
        row.get("fcm_token"),
        row.get("validated_at"),
        row.get("disabled_reason"),
    )
}

#[tokio::test]
async fn rechecks_only_due_tokens_and_acts_on_each_verdict() {
    let (_tempdir, database) = database_with(&[
        ("valid", "still-installed", 10),
        ("gone", "uninstalled", 10),
        ("unsure", "flaky", 10),
        ("recent", "recently-checked", 1_000),
    ])
    .await;
    let provider = ScriptedProvider::default();
    let counts = revalidate_due(
        &PushRegistrationRepository::new(database.pool().clone()),
        &provider,
        500,
        &mut None,
        &database.write_lock(),
        &AtomicBool::new(false),
    )
    .await
    .expect("revalidate");

    assert_eq!(
        counts,
        RevalidationCounts {
            validated: 1,
            disabled: 1,
            deferred: 1,
            purged: 0,
        }
    );
    let mut calls = provider.calls.lock().unwrap().clone();
    calls.sort();
    assert_eq!(calls, ["flaky", "still-installed", "uninstalled"]);

    let (_, validated_at, reason) = state_of(&database, "valid").await;
    assert!(1_000 < validated_at, "re-check moves validated_at to now");
    assert_eq!(reason, None);
    let (_, validated_at, reason) = state_of(&database, "gone").await;
    assert_eq!(
        (validated_at, reason.as_deref()),
        (10, Some("invalid_token"))
    );
    let (_, validated_at, reason) = state_of(&database, "unsure").await;
    assert_eq!(
        (validated_at, reason),
        (10, None),
        "an ambiguous answer changes nothing"
    );
    assert_eq!(state_of(&database, "recent").await.1, 1_000);
}

#[tokio::test]
async fn a_token_the_app_replaced_during_the_check_is_not_disabled() {
    let (tempdir, database) = database_with(&[("phone", "rotating", 10)]).await;
    let provider = ScriptedProvider {
        replaced_mid_check: Some(("rotating", database.clone())),
        ..ScriptedProvider::default()
    };
    revalidate_due(
        &PushRegistrationRepository::new(database.pool().clone()),
        &provider,
        500,
        &mut None,
        &database.write_lock(),
        &AtomicBool::new(false),
    )
    .await
    .expect("revalidate");
    let (token, _, reason) = state_of(&database, "phone").await;
    assert_eq!((token.as_str(), reason), ("new-token", None));
    drop(tempdir);
}

async fn insert_due(database: &Database, installation: &str, token: &str) {
    sqlx::query(
        "INSERT INTO push_registrations (
            recipient_id, installation_id, fcm_token, created_at, updated_at,
            last_seen_at, validated_at
         ) VALUES ('recipient', $1, $2, 1, 1, 1, 10)",
    )
    .bind(installation)
    .bind(token)
    .execute(database.pool())
    .await
    .expect("insert registration");
}

#[tokio::test]
async fn one_deferred_check_does_not_end_the_pass() {
    let (_tempdir, database) = database_with(&[]).await;
    insert_due(&database, "a-unsure", "flaky").await;
    for index in 0..REVALIDATION_BATCH {
        insert_due(
            &database,
            &format!("b-{index:03}"),
            &format!("token-{index}"),
        )
        .await;
    }
    insert_due(&database, "c-gone", "uninstalled").await;
    let mut resume_after = None;
    let counts = revalidate_due(
        &PushRegistrationRepository::new(database.pool().clone()),
        &ScriptedProvider::default(),
        500,
        &mut resume_after,
        &database.write_lock(),
        &AtomicBool::new(false),
    )
    .await
    .expect("revalidate");

    assert_eq!(
        counts,
        RevalidationCounts {
            validated: REVALIDATION_BATCH,
            disabled: 1,
            deferred: 1,
            purged: 0,
        }
    );
    assert!(resume_after.is_none(), "the pass reached the end");
}

#[tokio::test]
async fn rows_the_provider_keeps_deferring_do_not_starve_later_rows() {
    let (_tempdir, database) = database_with(&[]).await;
    for index in 0..REVALIDATION_BATCH {
        insert_due(
            &database,
            &format!("a-{index:03}"),
            &format!("flaky-{index}"),
        )
        .await;
    }
    insert_due(&database, "z-gone", "uninstalled").await;
    let registrations = PushRegistrationRepository::new(database.pool().clone());
    let provider = ScriptedProvider::default();
    let mut resume_after = None;

    let first = revalidate_due(
        &registrations,
        &provider,
        500,
        &mut resume_after,
        &database.write_lock(),
        &AtomicBool::new(false),
    )
    .await
    .expect("first pass");
    // A batch without a single definitive answer looks like an unavailable
    // provider, so the pass stops there.
    assert_eq!(first.deferred, REVALIDATION_BATCH);
    assert_eq!(state_of(&database, "z-gone").await.2, None);

    let second = revalidate_due(
        &registrations,
        &provider,
        500,
        &mut resume_after,
        &database.write_lock(),
        &AtomicBool::new(false),
    )
    .await
    .expect("second pass");
    assert_eq!((second.disabled, second.deferred), (1, 0));
    assert_eq!(
        state_of(&database, "z-gone").await.2.as_deref(),
        Some("invalid_token")
    );
    assert!(resume_after.is_none());
}

#[tokio::test]
async fn a_pass_deletes_only_registrations_disabled_longer_than_the_grace_period() {
    let (_tempdir, database) = database_with(&[]).await;
    let now = 100_000_000;
    let grace = i64::try_from(REJECTED_REGISTRATION_GRACE.as_secs()).unwrap();
    for (installation, disabled_at) in [
        ("expired", Some(now - grace - 1)),
        ("waiting", Some(now - grace + 1)),
        ("enabled", None),
    ] {
        sqlx::query(
            "INSERT INTO push_registrations (
                recipient_id, installation_id, fcm_token, created_at, updated_at,
                last_seen_at, validated_at, disabled_at, disabled_reason
             ) VALUES ('recipient', $1, $1, 1, 1, 1, $2, $3, $4)",
        )
        .bind(installation)
        .bind(now)
        .bind(disabled_at)
        .bind(disabled_at.map(|_| "invalid_token"))
        .execute(database.pool())
        .await
        .expect("insert registration");
    }

    let counts = run_pass(
        &PushRegistrationRepository::new(database.pool().clone()),
        &ScriptedProvider::default(),
        now,
        now - 1_000,
        &mut None,
        &database.write_lock(),
        &AtomicBool::new(false),
    )
    .await
    .expect("pass");

    assert_eq!(counts.purged, 1);
    let left: Vec<String> = sqlx::query_scalar(
        "SELECT installation_id FROM push_registrations ORDER BY installation_id",
    )
    .fetch_all(database.pool())
    .await
    .unwrap();
    assert_eq!(left, ["enabled", "waiting"]);
}
