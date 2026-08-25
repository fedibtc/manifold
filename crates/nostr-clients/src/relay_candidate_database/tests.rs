//! Tests for the stateless relay-candidate SDK database.

use nostr_sdk::{EventBuilder, Keys, Kind, Tag};

use super::*;

#[tokio::test]
async fn invalid_newer_addressable_event_cannot_suppress_older_candidate() {
    let keys = Keys::generate();
    let newer_invalid = EventBuilder::new(Kind::Custom(37_707), "invalid")
        .tag(Tag::identifier("setup-payment-federations"))
        .custom_created_at(nostr_sdk::Timestamp::from_secs(2))
        .sign_with_keys(&keys)
        .expect("newer test event signs");
    let older_valid_candidate = EventBuilder::new(
        Kind::Custom(37_707),
        r#"{"version":1,"fman_version":"0.1.0","federations":[],"telemetry_registration_url":"https://push.fedi.example/v1/telemetry/registrations"}"#,
    )
    .tag(Tag::identifier("setup-payment-federations"))
    .custom_created_at(nostr_sdk::Timestamp::from_secs(1))
    .sign_with_keys(&keys)
    .expect("older test event signs");
    let database = RelayCandidateDatabase;

    assert_eq!(
        database.save_event(&newer_invalid).await.unwrap(),
        SaveEventStatus::Success
    );
    assert_eq!(
        database.check_id(&older_valid_candidate.id).await.unwrap(),
        DatabaseEventStatus::NotExistent
    );
    assert_eq!(
        database.save_event(&older_valid_candidate).await.unwrap(),
        SaveEventStatus::Success
    );
}
