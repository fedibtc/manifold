use std::str::FromStr as _;
use std::time::Duration;

use fedi_decentralized_service_manifold_guardian::InviteCode;
use fedimint_core::PeerId;
use fedimint_core::config::FederationId;
use fedimint_core::db::IRawDatabaseExt as _;
use fedimint_core::db::mem_impl::MemDatabase;
use fedimint_core::invite_code::InviteCode as FedimintInviteCode;
use fedimint_core::runtime::Instant;
use fedimint_core::util::SafeUrl;

use super::{
    DriverRun, FormationRunOptions, ValueCallTimeoutBudget, combine_seat_invites,
    invite_federation_id, select_timer_duration,
};
use crate::FaError;
use crate::db::FaStore;

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
    let options = FormationRunOptions::default();
    let store = FaStore::new(MemDatabase::new().into_database());
    let lease = store
        .acquire_driver_lease(options.lease_duration(), options.lease_renewal_duration())
        .await
        .unwrap();
    let run = DriverRun::new(
        options,
        Instant::now() + Duration::from_nanos(999_999),
        &lease,
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
    let options = FormationRunOptions::default();
    let store = FaStore::new(MemDatabase::new().into_database());
    let lease = store
        .acquire_driver_lease(options.lease_duration(), options.lease_renewal_duration())
        .await
        .unwrap();
    let run = DriverRun::new(options, Instant::now() + Duration::from_secs(2), &lease);
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

#[tokio::test(start_paused = true)]
async fn wallet_call_stops_on_lease_takeover_before_its_deadline() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};

    let now = Arc::new(AtomicU64::new(1_000));
    let clock = {
        let now = now.clone();
        Arc::new(move || now.load(Ordering::SeqCst))
    };
    let store = FaStore::new_with_lease_clock(MemDatabase::new().into_database(), clock);
    let options = FormationRunOptions::default();
    let lease = store
        .acquire_driver_lease(options.lease_duration(), options.lease_renewal_duration())
        .await
        .unwrap();
    let run = DriverRun::new(options, Instant::now() + Duration::from_secs(600), &lease);
    let budget = run
        .prepare_value_call_budget("test wallet output")
        .await
        .unwrap();
    let completed = std::cell::Cell::new(false);
    let started = tokio::sync::Notify::new();
    let wallet = budget.poll_value_call(run, async {
        started.notify_one();
        fedimint_core::runtime::sleep(Duration::from_secs(200)).await;
        completed.set(true);
    });
    let takeover = async {
        started.notified().await;
        // Simulate process suspension beyond the lease without exhausting
        // the runtime deadline, then let a second driver acquire ownership.
        now.store(1_091, Ordering::SeqCst);
        store
            .acquire_driver_lease(options.lease_duration(), options.lease_renewal_duration())
            .await
            .unwrap()
    };
    let (result, replacement) = tokio::join!(wallet, takeover);
    assert!(matches!(result, Err(FaError::Busy)), "{result:?}");
    assert!(
        !completed.get(),
        "lost ownership cancels pending wallet work"
    );
    replacement.renew().await.unwrap();
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

fn seat_invite(federation_id: FederationId, peer: u16) -> InviteCode {
    InviteCode(
        FedimintInviteCode::new(
            SafeUrl::parse(&format!("https://guardian-{peer}.example/"))
                .expect("test URL is valid"),
            PeerId::from(peer),
            federation_id,
            None,
        )
        .to_string(),
    )
}

/// Seven seats tolerate two faulty guardians, so the delivered invite names
/// three: the lowest peer ids, whatever order the seats answered in. Taking
/// the first seat's invite gives one peer and taking every seat gives seven.
#[test]
fn the_delivered_invite_names_f_plus_one_guardians() {
    let federation_id: FederationId =
        "abababababababababababababababababababababababababababababababab"
            .parse()
            .unwrap();
    // Seats answer in an order unrelated to their peer ids.
    let invites = [5, 2, 6, 0, 3, 1, 4]
        .map(|peer| seat_invite(federation_id, peer))
        .to_vec();

    let combined = combine_seat_invites(&invites).unwrap();
    let parsed = FedimintInviteCode::from_str(&combined.0).unwrap();
    assert_eq!(parsed.federation_id(), federation_id);
    assert_eq!(parsed.api_secret(), None);
    let peers = parsed.peers();
    assert_eq!(
        peers.keys().map(|peer| peer.to_usize()).collect::<Vec<_>>(),
        [0, 1, 2]
    );
    for (peer, url) in &peers {
        assert_eq!(
            url.as_str(),
            format!("https://guardian-{}.example/", peer.to_usize())
        );
        // Each named guardian's own invite names the same federation.
        assert_eq!(
            invite_federation_id(&seat_invite(federation_id, peer.to_usize() as u16)).unwrap(),
            federation_id
        );
    }
    assert_eq!(invite_federation_id(&combined).unwrap(), federation_id);
    // The seats' answer order does not change the result.
    let mut reordered = invites.clone();
    reordered.reverse();
    assert_eq!(combine_seat_invites(&reordered).unwrap(), combined);

    // Four seats tolerate one fault: two guardians.
    let four = [3, 1, 0, 2]
        .map(|peer| seat_invite(federation_id, peer))
        .to_vec();
    let four = FedimintInviteCode::from_str(&combine_seat_invites(&four).unwrap().0).unwrap();
    assert_eq!(four.peers().len(), 2);
}

#[test]
fn seat_invites_must_cover_one_federation_once() {
    let federation_id: FederationId =
        "abababababababababababababababababababababababababababababababab"
            .parse()
            .unwrap();
    let other: FederationId = "cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd"
        .parse()
        .unwrap();
    let rejects = |invites: Vec<InviteCode>| {
        assert!(
            matches!(
                combine_seat_invites(&invites),
                Err(FaError::InvalidManifoldGuardians(_))
            ),
            "{invites:?}"
        );
    };
    // Another federation.
    rejects(vec![
        seat_invite(federation_id, 0),
        seat_invite(federation_id, 1),
        seat_invite(other, 2),
        seat_invite(federation_id, 3),
    ]);
    // The same guardian twice.
    rejects(vec![
        seat_invite(federation_id, 0),
        seat_invite(federation_id, 1),
        seat_invite(federation_id, 1),
        seat_invite(federation_id, 3),
    ]);
    // A peer id outside the seat count.
    rejects(vec![
        seat_invite(federation_id, 0),
        seat_invite(federation_id, 1),
        seat_invite(federation_id, 2),
        seat_invite(federation_id, 4),
    ]);
    assert!(combine_seat_invites(&[]).is_err());
}
