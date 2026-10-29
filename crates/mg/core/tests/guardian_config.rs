use std::collections::BTreeMap;

use fedimint_core::PeerId;
use fedimint_core::util::SafeUrl;
use fedimint_server::config::{PeerIrohEndpoints, ServerConfig};

use super::*;
use crate::test_support::write_installed_config;
use crate::transfer_native::tests::native_config;

/// A config in which this guardian is `identity` among peers 0..=3, its own
/// API endpoint keyed by `api_key`.
fn four_peer_config(identity: u16, api_key: u8) -> ServerConfig {
    let mut config = native_config();
    let template = config.consensus.iroh_endpoints[&PeerId::from(0)].clone();
    config.consensus.iroh_endpoints = (0..4)
        .map(|peer| {
            let key = if peer == identity {
                api_key
            } else {
                90 + peer as u8
            };
            (
                PeerId::from(peer),
                PeerIrohEndpoints {
                    name: format!("peer-{peer}"),
                    api_pk: iroh::SecretKey::from_bytes(&[key; 32])
                        .public()
                        .to_string()
                        .parse()
                        .unwrap(),
                    ..template.clone()
                },
            )
        })
        .collect::<BTreeMap<_, _>>();
    config.local.identity = PeerId::from(identity);
    config
}

/// The construction must be `fedimintd`'s byte for byte: the federation id
/// its `ServerConfig` calculates and the invite it hands out for its own
/// identity. A peer other than 0 catches an implementation that takes the
/// first endpoint instead of its own.
#[test]
fn installed_guardian_is_what_fedimintd_hands_out() {
    let config = four_peer_config(2, 7);
    let installed = InstalledGuardian::from_config(&config.consensus, &config.local).unwrap();
    let fedimintd = InviteCode::new(
        config.consensus.api_endpoints()[&config.local.identity]
            .url
            .clone(),
        config.local.identity,
        config.calculate_federation_id(),
        None,
    );
    assert_eq!(installed.invite, fedimintd);
    assert_eq!(installed.invite.to_string(), fedimintd.to_string());
    assert_eq!(
        installed.peer,
        FederationPeer {
            federation_id: config.calculate_federation_id(),
            peer_id: PeerId::from(2),
        }
    );
    assert_eq!(installed.invite.peer(), PeerId::from(2));
    assert_ne!(
        installed.invite.url(),
        config.consensus.api_endpoints()[&PeerId::from(0)].url
    );
}

/// The invite is built from whatever config is installed now, not from a
/// recorded string: an invite the child reported earlier for the same
/// guardian through another endpoint names the same formed fact but is not
/// what is handed out.
#[tokio::test]
async fn invite_follows_the_installed_endpoint() {
    let dir = tempfile::tempdir().unwrap();
    let config = native_config();
    let peer = FederationPeer {
        federation_id: config.calculate_federation_id(),
        peer_id: config.local.identity,
    };
    let earlier = InviteCode::new(
        SafeUrl::parse("wss://old-endpoint.example").unwrap(),
        peer.peer_id,
        peer.federation_id,
        None,
    );
    assert_eq!(FederationPeer::from_invite(&earlier), Some(peer));

    assert_eq!(
        InstalledGuardian::read_formed(dir.path(), &peer)
            .await
            .unwrap(),
        None,
        "no config, no invite"
    );
    write_installed_config(dir.path(), &config).await;
    let installed = InstalledGuardian::read_formed(dir.path(), &peer)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(installed.peer, peer);
    assert_ne!(installed.invite, earlier);
    assert_eq!(
        installed.invite.url().to_string(),
        format!(
            "iroh://{}",
            config.consensus.iroh_endpoints[&config.local.identity].api_pk
        )
    );
}

/// A config naming another guardian than the formed row never hands out
/// its invite.
#[tokio::test]
async fn config_for_another_guardian_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let config = four_peer_config(1, 7);
    write_installed_config(dir.path(), &config).await;
    let federation_id = config.calculate_federation_id();
    let other_peer = FederationPeer {
        federation_id,
        peer_id: PeerId::from(2),
    };
    assert!(
        InstalledGuardian::read_formed(dir.path(), &other_peer)
            .await
            .is_err()
    );
    let other_federation = FederationPeer {
        federation_id: four_peer_config(1, 8).calculate_federation_id(),
        peer_id: PeerId::from(1),
    };
    assert_ne!(other_federation.federation_id, federation_id);
    assert!(
        InstalledGuardian::read_formed(dir.path(), &other_federation)
            .await
            .is_err()
    );
}

/// Only an invite naming exactly one peer is one a seat's child handed out.
#[test]
fn only_single_guardian_invites_name_a_formed_peer() {
    let config = four_peer_config(0, 7);
    let federation_id = config.calculate_federation_id();
    let peers = config
        .consensus
        .api_endpoints()
        .into_iter()
        .map(|(peer, url)| (peer, url.url))
        .collect::<BTreeMap<_, _>>();
    let client_invite = InviteCode::new_with_essential_num_guardians(&peers, federation_id);
    assert!(client_invite.peers().len() > 1);
    assert_eq!(FederationPeer::from_invite(&client_invite), None);
}
