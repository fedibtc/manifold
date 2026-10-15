use fedi_decentralized_service_manifold_guardian::InviteCode;
use fedimint_core::PeerId;
use fedimint_core::config::FederationId;
use fedimint_core::invite_code::InviteCode as FedimintInviteCode;
use fedimint_core::util::SafeUrl;

use super::invite_federation_id;
use crate::FaError;

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
