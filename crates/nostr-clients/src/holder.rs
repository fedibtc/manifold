//! Holder-facing Nostr service trait and SDK-backed client.

use nostr_sdk::{EventId, Kind, PublicKey, Tag};

use crate::{NostrClientResult, NostrRelayClient};

/// Request for publishing a Holder-published FMan authorization event.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublishFmanAuthorizationRequest {
    /// FMan Nostr pubkey targeted by the authorization.
    pub fman_pubkey: PublicKey,

    /// Credential issuer authority pubkey used as an indexing hint.
    pub issuer_pubkey: String,

    /// Digest of the credential authorized for FMan use.
    pub credential_digest: String,

    /// Credential schema used as an indexing hint.
    pub schema: String,

    /// Canonical JSON content of the Holder-published authorization envelope.
    pub content: String,
}

/// Request for publishing a Holder-published FLIP provider authorization event.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublishFlipAuthorizationRequest {
    /// FLIP provider Nostr pubkey targeted by the authorization.
    pub provider_pubkey: PublicKey,

    /// Digest of the credential authorized for FLIP use.
    pub credential_digest: String,

    /// Canonical JSON content of the Holder-published authorization envelope.
    pub content: String,
}

/// Nostr interactions performed by the Holder component.
pub trait HolderNostrClient {
    /// Publish an authorization event that allows an FMan to present a Holder credential.
    ///
    /// The `d` tag is derived from the FMan pubkey and credential digest to keep the
    /// addressable event index consistent with the explicit request fields. The
    /// published event is authored by the keys configured on the underlying relay
    /// client, and callers must keep the envelope content consistent with those keys.
    ///
    /// # Errors
    ///
    /// Returns an error if publishing fails or no configured relay accepts
    /// the event. On a pooled client one relay's acceptance is success;
    /// other relays' rejections are logged, not returned.
    async fn publish_fman_authorization(
        &self,
        request: PublishFmanAuthorizationRequest,
    ) -> NostrClientResult<EventId>;

    /// Publish an authorization event that allows a FLIP provider to present a
    /// Holder credential.
    ///
    /// The authorization, event kind and envelope content are the FMan variant's;
    /// only the coordinate that addresses it differs. A FLIP provider fetches with
    /// [`FLIP_AUTHORIZATION_HASHTAG`], so an FMan-tagged event is never a candidate
    /// for it however correctly it names the provider in `p` — publishing both is
    /// what lets one Holder credential serve both components.
    ///
    /// [`FLIP_AUTHORIZATION_HASHTAG`]: fedi_decentralized_nostr::flip::FLIP_AUTHORIZATION_HASHTAG
    ///
    /// # Errors
    ///
    /// Returns an error if publishing fails or no configured relay accepts
    /// the event. On a pooled client one relay's acceptance is success;
    /// other relays' rejections are logged, not returned.
    async fn publish_flip_authorization(
        &self,
        request: PublishFlipAuthorizationRequest,
    ) -> NostrClientResult<EventId>;
}

/// Nostr SDK implementation of [`HolderNostrClient`].
#[derive(Clone, Debug)]
pub struct NostrHolderClient {
    /// Shared relay client used to publish holder events.
    relay: NostrRelayClient,
}

impl NostrHolderClient {
    /// Create a Holder client from a shared relay client.
    pub fn new(relay: NostrRelayClient) -> Self {
        Self { relay }
    }
}

impl HolderNostrClient for NostrHolderClient {
    async fn publish_fman_authorization(
        &self,
        request: PublishFmanAuthorizationRequest,
    ) -> NostrClientResult<EventId> {
        let fman_pubkey = request.fman_pubkey.to_string();
        let authorization_d_tag = fedi_decentralized_nostr::fman::fman_authorization_d_tag(
            &fman_pubkey,
            &request.credential_digest,
        );

        self.relay
            .publish_event(
                nostr_sdk::EventBuilder::new(
                    Kind::Custom(fedi_decentralized_nostr::fman::HOLDER_AUTHORIZATION_EVENT_KIND),
                    request.content,
                )
                .tags([
                    tag(["d", authorization_d_tag.as_str()]),
                    tag([
                        "t",
                        fedi_decentralized_nostr::fman::FMAN_AUTHORIZATION_HASHTAG,
                    ]),
                    tag(["p", fman_pubkey.as_str()]),
                    tag(["issuer", request.issuer_pubkey.as_str()]),
                    tag(["credential", request.credential_digest.as_str()]),
                    tag(["schema", request.schema.as_str()]),
                ]),
            )
            .await
    }

    async fn publish_flip_authorization(
        &self,
        request: PublishFlipAuthorizationRequest,
    ) -> NostrClientResult<EventId> {
        let provider_pubkey = request.provider_pubkey.to_string();
        let authorization_d_tag = fedi_decentralized_nostr::flip::flip_authorization_d_tag(
            &provider_pubkey,
            &request.credential_digest,
        );

        // Only d/t/p: those are the coordinate a FLIP provider fetches on, and
        // its verification reads the envelope rather than any indexing hint. The
        // FMan variant's issuer/credential/schema tags are not repeated here so
        // this event carries nothing a consumer is not documented to read.
        self.relay
            .publish_event(
                nostr_sdk::EventBuilder::new(
                    Kind::Custom(fedi_decentralized_nostr::flip::HOLDER_AUTHORIZATION_EVENT_KIND),
                    request.content,
                )
                .tags([
                    tag(["d", authorization_d_tag.as_str()]),
                    tag([
                        "t",
                        fedi_decentralized_nostr::flip::FLIP_AUTHORIZATION_HASHTAG,
                    ]),
                    tag(["p", provider_pubkey.as_str()]),
                ]),
            )
            .await
    }
}

/// Build a Nostr tag from static-sized string slices.
fn tag<const N: usize>(values: [&str; N]) -> Tag {
    Tag::parse(values).expect("Nostr client tag construction uses valid tags")
}
