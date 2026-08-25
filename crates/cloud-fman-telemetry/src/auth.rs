use axum::http::Method;
use base64::{Engine as _, engine::general_purpose};
use nostr::Event;
use serde_json::Value;
use sha2::{Digest as _, Sha256};

/// Verified signed request identity and durable replay fields.
#[derive(Clone, Debug)]
pub(crate) struct VerifiedHttpAuth {
    pub(crate) signer: String,
    pub(crate) event_id: String,
    pub(crate) created_at: i64,
}

/// Verify an exact-body NIP-98 authorization without retaining rejected input.
pub(crate) fn verify(
    header: &str,
    method: &Method,
    url: &str,
    body: &[u8],
    now: i64,
) -> Result<VerifiedHttpAuth, AuthError> {
    let encoded = header.strip_prefix("Nostr ").ok_or(AuthError)?;
    let decoded = general_purpose::STANDARD
        .decode(encoded)
        .map_err(|_| AuthError)?;
    let value: Value = serde_json::from_slice(&decoded).map_err(|_| AuthError)?;
    let raw_pubkey = value
        .get("pubkey")
        .and_then(Value::as_str)
        .ok_or(AuthError)?
        .to_owned();
    let event: Event = serde_json::from_value(value).map_err(|_| AuthError)?;
    if event.kind != nostr::Kind::HttpAuth || raw_pubkey != event.pubkey.to_string() {
        return Err(AuthError);
    }
    event.verify().map_err(|_| AuthError)?;
    if tag(&event, "u") != Some(url) || tag(&event, "method") != Some(method.as_str()) {
        return Err(AuthError);
    }
    let hash = hex::encode(Sha256::digest(body));
    if tag(&event, "payload") != Some(hash.as_str()) {
        return Err(AuthError);
    }
    let created_at = i64::try_from(event.created_at.as_secs()).map_err(|_| AuthError)?;
    if created_at < now - 60 || created_at > now + 5 {
        return Err(AuthError);
    }
    Ok(VerifiedHttpAuth {
        signer: event.pubkey.to_string(),
        event_id: event.id.to_string(),
        created_at,
    })
}

fn tag<'a>(event: &'a Event, name: &str) -> Option<&'a str> {
    event.tags.iter().find_map(|tag| {
        let values = tag.as_slice();
        (values.len() >= 2 && values[0] == name).then_some(values[1].as_str())
    })
}

/// Uniform authorization failure with no rejected material.
#[derive(Debug, thiserror::Error)]
#[error("registration authorization refused")]
pub(crate) struct AuthError;

#[cfg(test)]
mod tests {
    use super::*;
    use base64::engine::general_purpose;
    use nostr::{EventBuilder, Keys, Kind, Tag, Timestamp};

    fn authorization(keys: &Keys, url: &str, body: &[u8], timestamp: u64) -> String {
        let payload = hex::encode(Sha256::digest(body));
        let event = EventBuilder::new(Kind::HttpAuth, "")
            .custom_created_at(Timestamp::from(timestamp))
            .tag(Tag::parse(["u", url]).unwrap())
            .tag(Tag::parse(["method", "POST"]).unwrap())
            .tag(Tag::parse(["payload", &payload]).unwrap())
            .sign_with_keys(keys)
            .unwrap();
        format!(
            "Nostr {}",
            general_purpose::STANDARD.encode(serde_json::to_vec(&event).unwrap())
        )
    }

    #[test]
    fn exact_url_body_signature_and_freshness_are_required() {
        let keys = Keys::generate();
        let body = br#"{"generation":7}"#;
        let header = authorization(
            &keys,
            "https://collector.test/v1/telemetry/registrations",
            body,
            100,
        );
        let verified = verify(
            &header,
            &Method::POST,
            "https://collector.test/v1/telemetry/registrations",
            body,
            100,
        )
        .unwrap();
        assert_eq!(verified.signer, keys.public_key().to_string());
        assert!(verify(&header, &Method::POST, "https://wrong.test/", body, 100).is_err());
        assert!(
            verify(
                &header,
                &Method::POST,
                "https://collector.test/v1/telemetry/registrations",
                b"changed",
                100
            )
            .is_err()
        );
        assert!(
            verify(
                &header,
                &Method::POST,
                "https://collector.test/v1/telemetry/registrations",
                body,
                161
            )
            .is_err()
        );
    }

    #[test]
    fn malformed_or_tampered_authorization_is_sanitized() {
        let error = verify(
            "Nostr not-base64",
            &Method::POST,
            "https://collector.test/",
            b"",
            1,
        )
        .unwrap_err();
        assert_eq!(error.to_string(), "registration authorization refused");
        assert!(!format!("{error:?}").contains("not-base64"));
    }
}
