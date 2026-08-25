//! Nostr registry constants for FLIP liquidity-provider advertisements and
//! the Holder authorizations that vouch for a provider identity.

/// Provisional FLIP provider advertisement event kind.
pub const FLIP_PROVIDER_ADVERTISEMENT_EVENT_KIND: u16 = 37702;

/// Addressable-event identifier for the current provider advertisement.
pub const FLIP_PROVIDER_ADVERTISEMENT_D_TAG: &str = "flip-provider-ad";

/// Hashtag used to enumerate FLIP providers without filtering by author.
pub const FLIP_PROVIDER_ADVERTISEMENT_HASHTAG: &str = "fedi-flip";

/// Holder-published FLIP provider authorization event kind.
///
/// Deliberately the same kind as [`crate::fman::HOLDER_AUTHORIZATION_EVENT_KIND`],
/// and defined in terms of it so the two cannot drift apart. The document is
/// one Holder authorization over a service subject; only the addressing
/// differs. A FLIP-targeted event carries the FLIP `d` prefix and hashtag
/// below and a `p` tag naming the provider, so an FMan's kind + `p` + `t`
/// filter can never match one, and neither can this one match an FMan's.
pub const HOLDER_AUTHORIZATION_EVENT_KIND: u16 = crate::fman::HOLDER_AUTHORIZATION_EVENT_KIND;

/// Hashtag used to index holder-published FLIP provider authorizations.
pub const FLIP_AUTHORIZATION_HASHTAG: &str = "fedi-flip-authorization";

/// Prefix for holder-published FLIP provider authorization `d` tags.
pub const FLIP_AUTHORIZATION_D_TAG_PREFIX: &str = "flip-authorization";

/// Build the `d` tag value for a holder-published FLIP provider authorization.
///
/// The subject pubkey is part of the coordinate, so one Holder authorizing
/// several providers with one credential publishes one addressable event per
/// provider rather than replacing its own previous authorization.
///
/// `credential_digest` is the credential-SDK `CredentialDigest` wire form, the
/// same encoding the FMan variant and the attester revocation `d` tag use.
/// Publishers and fetchers must agree on it or addressable replacement
/// diverges across components.
#[must_use]
pub fn flip_authorization_d_tag(provider_pubkey: &str, credential_digest: &str) -> String {
    format!("{FLIP_AUTHORIZATION_D_TAG_PREFIX}:{provider_pubkey}:{credential_digest}")
}

/// Signed content of a Holder-published FLIP provider authorization event.
///
/// Re-exported rather than redefined: the content schema names no service
/// role, so FLIP and FMan share one typed rendering and cannot disagree on the
/// wire shape. Only the addressing above is FLIP's own. The outer Nostr event
/// author must still be checked against `holder_id_pubkey`.
pub use crate::fman::HolderAuthorizationEventContent;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flip_authorization_d_tag_uses_prefix_subject_and_digest() {
        assert_eq!(
            flip_authorization_d_tag(
                "3bf0c63fcb93463407af97a5e5ee64fa883d107ef9e558472c4eb9aaaefa459d",
                "2u0Za9RCXVW0zzoUpG-4iBGCLGVdnvBpJUZoGDaK5dY"
            ),
            "flip-authorization:\
             3bf0c63fcb93463407af97a5e5ee64fa883d107ef9e558472c4eb9aaaefa459d:\
             2u0Za9RCXVW0zzoUpG-4iBGCLGVdnvBpJUZoGDaK5dY"
        );
    }

    #[test]
    fn flip_and_fman_authorizations_share_a_kind_but_not_an_index() {
        assert_eq!(
            HOLDER_AUTHORIZATION_EVENT_KIND,
            crate::fman::HOLDER_AUTHORIZATION_EVENT_KIND
        );
        assert_ne!(
            FLIP_AUTHORIZATION_HASHTAG,
            crate::fman::FMAN_AUTHORIZATION_HASHTAG
        );
        assert_ne!(
            FLIP_AUTHORIZATION_D_TAG_PREFIX,
            crate::fman::FMAN_AUTHORIZATION_D_TAG_PREFIX
        );
    }
}
