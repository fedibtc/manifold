/// The config-revision fence is exactly this: the hash the client actually
/// carries must equal the hash the request was accepted against.
///
/// A regression that normalised either side — trimming, case folding, prefix
/// matching — would keep every other test green while letting a client with a
/// different module map mint the address the provider funds.
#[test]
fn the_config_hash_fence_is_an_exact_comparison() {
    let hash = "a3f1".repeat(16);

    assert!(
        matches!(config_hash_check(&hash, &hash), TargetCheck::Usable),
        "the accepted hash is usable"
    );

    // Different value.
    let other = "b4e2".repeat(16);
    let TargetCheck::Unusable(reason) = config_hash_check(&other, &hash) else {
        panic!("a different config hash must be refused");
    };
    assert!(
        reason.contains(&other) && reason.contains(&hash),
        "the refusal names both hashes so an operator can tell them apart: {reason}"
    );

    // Same bytes, different case. `hex::encode` emits lowercase, so an
    // uppercase observation means something re-encoded it, and the fence
    // must not accept it silently.
    assert!(
        matches!(
            config_hash_check(&hash.to_uppercase(), &hash),
            TargetCheck::Unusable(_)
        ),
        "case folding would weaken the fence"
    );

    // Whitespace and prefixes, the two other normalisations that would look
    // harmless.
    assert!(
        matches!(
            config_hash_check(&format!(" {hash} "), &hash),
            TargetCheck::Unusable(_)
        ),
        "trimming would weaken the fence"
    );
    assert!(
        matches!(
            config_hash_check(&hash[..hash.len() - 2], &hash),
            TargetCheck::Unusable(_)
        ),
        "a prefix is not a match"
    );
}
use fedi_decentralized_service_liquidity_manager::Url;

use super::*;

/// An esplora-configured daemon hands its own backend to target clients;
/// a bitcoind-configured one has nothing the Fedimint wallet client can
/// use and must leave it on what the target federation advertises. The
/// second case reads like an oversight, so it is pinned deliberately.
#[test]
fn only_an_esplora_observer_can_back_a_target_client() {
    let esplora = ChainObserverConfigView {
        backend: ChainObserverBackendView::Esplora {
            url: Url("http://127.0.0.1:3002".to_owned()),
        },
    };
    assert_eq!(
        target_client_esplora_url(&esplora).map(|url| url.to_string()),
        Some("http://127.0.0.1:3002/".to_owned())
    );

    let bitcoind = ChainObserverConfigView {
        backend: ChainObserverBackendView::Bitcoind {
            url: Url("http://127.0.0.1:18443".to_owned()),
            username: Some("user".to_owned()),
            has_password: true,
        },
    };
    assert_eq!(target_client_esplora_url(&bitcoind), None);
}

/// A URL the client cannot parse is not a reason to refuse to fund: the
/// target federation's advertised backend is still there to fall back on.
#[test]
fn an_unparsable_esplora_url_falls_back_rather_than_failing() {
    let broken = ChainObserverConfigView {
        backend: ChainObserverBackendView::Esplora {
            url: Url("not a url".to_owned()),
        },
    };
    assert_eq!(target_client_esplora_url(&broken), None);
}

/// The gate that keeps a stability allocation off a federation without the
/// module reads a spelled-out kind, so a silent upstream rename would turn
/// it into a check that always fails and rejects every target.
#[test]
fn the_required_module_kind_is_the_one_upstream_registers() {
    assert_eq!(
        STABILITY_POOL_MODULE_KIND,
        stability_pool_client::common::KIND.as_str()
    );
}
