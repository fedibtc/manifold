//! Shared attester Nostr event kinds and tags.
//!
//! These constants pin the shared FI/FMan/FLIP issuer publication profile:
//! issuer authorities and credential revocations are issuer-authored
//! addressable events that the shared verifier fetches from the relays
//! selected by its environment and current authenticated authority.

/// Issuer-authority distribution event kind.
///
/// Addressable, provisional kind used by issuers to distribute their current
/// `IssuerAuthority` identity document. Valid only when the event author
/// equals `issuer.issuer_id_pubkey` and `IssuerAuthority::verify()` passes.
pub const ISSUER_AUTHORITY_EVENT_KIND: u16 = 37703;

/// `d` tag value used for issuer-authority events.
pub const ISSUER_AUTHORITY_D_TAG: &str = "issuer-authority";

/// Hashtag used to index issuer-authority events.
pub const ISSUER_AUTHORITY_HASHTAG: &str = "fedi-attester-issuer";

/// `RevocationLocation::protocol` value naming a Nostr relay location.
/// Producers of authority documents and the verifier's revocation-location
/// filter must agree on this string exactly.
pub const NOSTR_REVOCATION_LOCATION_PROTOCOL: &str = "nostr";

/// Issuer credential-revocation event kind.
///
/// Addressable, provisional kind used by issuers to publish
/// `fedi-credential-sdk-protocol::SignedRevocation` documents. Verifiers fetch
/// these from every relay location listed in the authenticated
/// `IssuerAuthority.issuer.revocation` entries and must verify the signed
/// content. The authority delegates negative-state completeness trust to those
/// relay operators; tags remain indexing hints only.
pub const CREDENTIAL_REVOCATION_EVENT_KIND: u16 = 37704;

/// Hashtag used to index attester credential revocations.
pub const CREDENTIAL_REVOCATION_HASHTAG: &str = "fedi-credential-revocation";

/// Prefix for attester credential revocation `d` tags.
pub const CREDENTIAL_REVOCATION_D_TAG_PREFIX: &str = "credential-revocation";

/// Build the `d` tag value for an attester credential revocation event.
///
/// `credential_digest` is the credential-SDK `CredentialDigest` wire form: the
/// base64url-unpadded SHA-256 digest string produced by `Credential::digest()`
/// serde serialization. Publishers and fetchers must use this same encoding or
/// addressable replacement/filtering diverges across components.
pub fn credential_revocation_d_tag(credential_digest: &str) -> String {
    format!("{CREDENTIAL_REVOCATION_D_TAG_PREFIX}:{credential_digest}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credential_revocation_d_tag_uses_prefix_and_digest() {
        assert_eq!(
            credential_revocation_d_tag("2u0Za9RCXVW0zzoUpG-4iBGCLGVdnvBpJUZoGDaK5dY"),
            "credential-revocation:2u0Za9RCXVW0zzoUpG-4iBGCLGVdnvBpJUZoGDaK5dY"
        );
    }
}
