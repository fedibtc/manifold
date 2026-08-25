use super::*;
use fedimint_core::PeerId;
use fedimint_core::config::FederationId;

const CANONICAL_IROH_NODE_ID: &str =
    "ae58ff8833241ac82d6ff7611046ed67b5072d142c588d0063e942d9a75502b6";

fn invite_to(url: &str) -> String {
    InviteCode::new(
        SafeUrl::parse(url).expect("test url"),
        PeerId::from(0),
        FederationId::dummy(),
        None,
    )
    .to_string()
}

#[tokio::test]
async fn malformed_invite_is_refused() {
    assert_eq!(
        check_invite_endpoints(EndpointPolicy::GlobalOnly, "not-an-invite")
            .await
            .expect_err("malformed invite"),
        EndpointPolicyError::MalformedInvite
    );
}

#[tokio::test]
async fn canonical_iroh_endpoint_passes_under_both_policies() {
    let invite = invite_to(&format!("iroh://{CANONICAL_IROH_NODE_ID}"));
    for policy in [EndpointPolicy::GlobalOnly, EndpointPolicy::AllowPrivate] {
        check_invite_endpoints(policy, &invite)
            .await
            .expect("canonical Iroh endpoint");
    }
}

#[test]
fn malformed_or_noncanonical_iroh_endpoints_are_refused() {
    let base32_node_id = "vzmp7cbteqnmqllp65qrarxnm62qoliufrmi2add5fbntj2vak3a";
    for url in [
        "iroh://not-a-node-id".to_owned(),
        format!("iroh://{base32_node_id}"),
        format!("iroh://user@{CANONICAL_IROH_NODE_ID}"),
        format!("iroh://{CANONICAL_IROH_NODE_ID}:443"),
        format!("iroh://{CANONICAL_IROH_NODE_ID}/guardian"),
        format!("iroh://{CANONICAL_IROH_NODE_ID}?relay=example.com"),
        format!("iroh://{CANONICAL_IROH_NODE_ID}#fragment"),
    ] {
        assert_eq!(
            check_iroh_endpoint(&SafeUrl::parse(&url).expect("syntactic test URL"))
                .expect_err("noncanonical Iroh URL"),
            EndpointPolicyError::InvalidIrohEndpoint,
            "{url}"
        );
    }
}

#[tokio::test]
async fn global_only_refuses_every_websocket_form_before_network_work() {
    for url in [
        "ws://attacker.invalid:18173/",
        "wss://attacker.invalid:443/",
        "ws://93.184.216.34:18173/",
        "wss://93.184.216.34:443/",
        "ws://127.0.0.1:18173/",
        "wss://[::1]:443/",
        "ws://[64:ff9b::7f00:1]:18173/",
        "ws://[2002:7f00:1::]:18173/",
        "ws://[2001:0000:4136:e378:8000:63bf:3fff:fdd2]:18173/",
    ] {
        assert_eq!(
            check_invite_endpoints(EndpointPolicy::GlobalOnly, &invite_to(url))
                .await
                .expect_err("WebSocket must fail closed"),
            EndpointPolicyError::WebSocketDisallowed,
            "{url}"
        );
    }
}

#[tokio::test]
async fn private_allowance_permits_websocket_names_and_literals() {
    for url in [
        "ws://guardian.test:18173/",
        "wss://guardian.test:443/",
        "ws://93.184.216.34:18173/",
        "wss://127.0.0.1:443/",
    ] {
        check_invite_endpoints(EndpointPolicy::AllowPrivate, &invite_to(url))
            .await
            .expect("operator explicitly allowed WebSocket endpoints");
    }
}

#[tokio::test]
async fn one_iroh_peer_cannot_bless_a_websocket_peer() {
    #[derive(fedimint_core::encoding::Encodable)]
    enum RawInvitePart {
        Api { url: SafeUrl, peer: PeerId },
        FederationId(FederationId),
    }
    use fedimint_core::base32::FEDIMINT_PREFIX;
    let invite = fedimint_core::base32::encode_prefixed(
        FEDIMINT_PREFIX,
        &vec![
            RawInvitePart::Api {
                url: SafeUrl::parse(&format!("iroh://{CANONICAL_IROH_NODE_ID}"))
                    .expect("test Iroh URL"),
                peer: PeerId::from(0),
            },
            RawInvitePart::Api {
                url: SafeUrl::parse("ws://93.184.216.34:18173/").expect("test WebSocket URL"),
                peer: PeerId::from(1),
            },
            RawInvitePart::FederationId(FederationId::dummy()),
        ],
    );
    assert_eq!(
        check_invite_endpoints(EndpointPolicy::GlobalOnly, &invite)
            .await
            .expect_err("every peer must pass"),
        EndpointPolicyError::WebSocketDisallowed
    );
}

#[tokio::test]
async fn endpoint_cardinality_is_bounded_before_transport_work() {
    #[derive(fedimint_core::encoding::Encodable)]
    enum RawInvitePart {
        Api { url: SafeUrl, peer: PeerId },
        FederationId(FederationId),
    }

    use fedimint_core::base32::FEDIMINT_PREFIX;
    let mut parts = (0..=MAX_INVITE_ENDPOINTS)
        .map(|index| RawInvitePart::Api {
            url: SafeUrl::parse(&format!("iroh://{CANONICAL_IROH_NODE_ID}")).expect("test URL"),
            peer: PeerId::from(index as u16),
        })
        .collect::<Vec<_>>();
    parts.push(RawInvitePart::FederationId(FederationId::dummy()));
    let invite = fedimint_core::base32::encode_prefixed(FEDIMINT_PREFIX, &parts);
    assert_eq!(
        check_invite_endpoints(EndpointPolicy::GlobalOnly, &invite)
            .await
            .expect_err("endpoint limit"),
        EndpointPolicyError::TooManyEndpoints
    );
}

#[tokio::test]
async fn unsupported_scheme_is_refused_under_both_policies() {
    for policy in [EndpointPolicy::GlobalOnly, EndpointPolicy::AllowPrivate] {
        assert_eq!(
            check_invite_endpoints(policy, &invite_to("http://93.184.216.34/"))
                .await
                .expect_err("unsupported scheme"),
            EndpointPolicyError::UnsupportedScheme
        );
    }
}
