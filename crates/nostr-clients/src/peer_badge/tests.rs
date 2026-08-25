use std::sync::{Arc, Mutex};

use nostr_sdk::{EventBuilder, Keys};

use super::*;

#[tokio::test]
async fn revocation_candidates_are_combined_from_every_location() {
    let relays = [
        RelayUrl::parse("wss://first.example").expect("first relay"),
        RelayUrl::parse("wss://second.example").expect("second relay"),
    ];
    let revocation = EventBuilder::text_note("revoked")
        .sign_with_keys(&Keys::generate())
        .expect("sign test event");
    let expected_revocation = revocation.clone();
    let first_relay = relays[0].clone();
    let visited = Arc::new(Mutex::new(Vec::new()));
    let candidates = fetch_all_relay_candidates(&relays, {
        let visited = Arc::clone(&visited);
        move |relay| {
            let visited = Arc::clone(&visited);
            let revocation = revocation.clone();
            let first_relay = first_relay.clone();
            async move {
                visited.lock().expect("visited lock").push(relay.clone());
                if relay == first_relay {
                    Ok(Vec::new())
                } else {
                    Ok(vec![revocation])
                }
            }
        }
    })
    .await
    .expect("both complete relay results combine");

    assert_eq!(*visited.lock().expect("visited lock"), relays);
    assert_eq!(candidates, [expected_revocation]);
}

#[tokio::test]
async fn any_incomplete_revocation_location_fails_the_combined_lookup() {
    let relays = [
        RelayUrl::parse("wss://first.example").expect("first relay"),
        RelayUrl::parse("wss://second.example").expect("second relay"),
    ];
    let first_relay = relays[0].clone();
    let visited = Arc::new(Mutex::new(Vec::new()));
    let result = fetch_all_relay_candidates(&relays, {
        let visited = Arc::clone(&visited);
        move |relay| {
            let visited = Arc::clone(&visited);
            let first_relay = first_relay.clone();
            async move {
                visited.lock().expect("visited lock").push(relay.clone());
                if relay == first_relay {
                    Ok(Vec::new())
                } else {
                    Err(NostrClientError::IncompleteQuery {
                        reason: "test incomplete result",
                    })
                }
            }
        }
    })
    .await;

    assert!(matches!(
        result,
        Err(NostrClientError::IncompleteQuery { .. })
    ));
    assert_eq!(*visited.lock().expect("visited lock"), relays);
}
