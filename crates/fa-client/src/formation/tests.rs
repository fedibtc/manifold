use std::time::Duration;

use fedi_decentralized_service_manifold_guardian::InviteCode;
use fedimint_core::PeerId;
use fedimint_core::config::FederationId;
use fedimint_core::invite_code::InviteCode as FedimintInviteCode;
use fedimint_core::runtime::Instant;
use fedimint_core::util::SafeUrl;

use super::{
    DriverRun, FormationRunOptions, ValueCallTimeoutBudget, invite_federation_id,
    select_timer_duration,
};
use crate::FaError;

#[test]
fn effective_runtime_timer_rejects_sub_millisecond_remainders() {
    assert!(matches!(
        select_timer_duration(
            Duration::from_millis(1),
            Duration::from_nanos(999_999),
            "test"
        ),
        Err(FaError::Timeout(operation)) if operation == "test"
    ));
    assert_eq!(
        select_timer_duration(Duration::from_millis(1), Duration::from_millis(2), "test")
            .expect("one millisecond is an effective runtime timer"),
        Duration::from_millis(1)
    );
    assert_eq!(
        select_timer_duration(Duration::from_millis(2), Duration::from_millis(1), "test")
            .expect("one millisecond run remainder is an effective runtime timer"),
        Duration::from_millis(1)
    );
}

#[tokio::test]
async fn sub_millisecond_remainder_precedes_construct_effect() {
    let run = DriverRun::with_deadline(
        FormationRunOptions::default(),
        Instant::now() + Duration::from_nanos(999_999),
    );
    let constructed = std::cell::Cell::new(false);

    assert!(matches!(
        run.construct("test signing", || {
            constructed.set(true);
            Ok(())
        })
        .await,
        Err(FaError::Timeout(operation)) if operation == "test signing"
    ));
    assert!(!constructed.get());
}

#[tokio::test(start_paused = true)]
async fn value_call_budget_rechecks_the_absolute_deadline_when_polled() {
    let run = DriverRun::with_deadline(
        FormationRunOptions::default(),
        Instant::now() + Duration::from_secs(2),
    );
    let budget = ValueCallTimeoutBudget {
        operation: "test wallet output",
        deadline: Instant::now() + Duration::from_secs(2),
    };
    tokio::time::advance(Duration::from_secs(3)).await;
    let polled = std::cell::Cell::new(false);

    let result = budget
        .poll_value_call(run, async {
            polled.set(true);
        })
        .await;

    assert!(
        matches!(result, Err(FaError::Timeout(operation)) if operation == "test wallet output")
    );
    assert!(
        !polled.get(),
        "an expired wallet future must remain unpolled"
    );
}

#[test]
fn invite_identity_is_a_concrete_federation_id() {
    let federation_id: FederationId =
        "abababababababababababababababababababababababababababababababab"
            .parse()
            .expect("test federation ID is valid");
    let first = invite(federation_id, "https://first.example/", None);
    let second = invite(federation_id, "https://second.example/", None);
    let different = invite(
        "cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd"
            .parse()
            .expect("test federation ID is valid"),
        "https://first.example/",
        None,
    );

    let parsed: FederationId = invite_federation_id(&first).expect("public invite identity parses");
    assert_eq!(parsed, invite_federation_id(&second).unwrap());
    assert_ne!(parsed, invite_federation_id(&different).unwrap());
}

#[test]
fn invite_identity_rejects_invalid_and_bearer_secret_invites() {
    assert!(matches!(
        invite_federation_id(&InviteCode("not an invite".to_owned())),
        Err(FaError::InvalidManifoldGuardians(message))
            if message.contains("invalid federation invite")
    ));

    let private = invite(
        "abababababababababababababababababababababababababababababababab"
            .parse()
            .expect("test federation ID is valid"),
        "https://private.example/",
        Some("secret".to_owned()),
    );
    assert!(matches!(
        invite_federation_id(&private),
        Err(FaError::InvalidManifoldGuardians(message))
            if message.contains("bearer-secret federation invite")
    ));
}

fn invite(federation_id: FederationId, url: &str, api_secret: Option<String>) -> InviteCode {
    InviteCode(
        FedimintInviteCode::new(
            SafeUrl::parse(url).expect("test URL is valid"),
            PeerId::from(0),
            federation_id,
            api_secret,
        )
        .to_string(),
    )
}
