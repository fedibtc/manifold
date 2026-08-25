//! Error types for Nostr client service traits.

use nostr_sdk::RelayUrl;

/// Result type returned by Nostr client service traits.
pub type NostrClientResult<T> = Result<T, NostrClientError>;

/// Error returned while connecting to relays or publishing and fetching events.
#[derive(Debug, thiserror::Error)]
pub enum NostrClientError {
    /// A relay URL failed to parse.
    #[error("invalid relay URL `{url}`")]
    InvalidRelayUrl {
        /// Relay URL that failed to parse.
        url: String,

        /// Underlying parser error.
        reason: String,
    },

    /// Adding a relay to a client failed.
    #[error("failed to add relay `{url}`")]
    AddRelay {
        /// Relay URL that could not be added.
        url: RelayUrl,

        /// Underlying client error.
        source: nostr_sdk::client::Error,
    },

    /// No relay accepted a client connection.
    #[error("failed to connect to any configured Nostr relay")]
    Connect,

    /// Sending an event failed before relay acknowledgement.
    #[error("failed to publish Nostr event")]
    Publish {
        /// Underlying client error.
        source: nostr_sdk::client::Error,
    },

    /// No relay accepted a published event.
    #[error("failed relay publish acknowledgement: {failed}")]
    PublishRejected {
        /// Debug details for failed relay acknowledgements.
        failed: String,
    },

    /// Fetching events failed.
    #[error("failed to fetch Nostr events")]
    Fetch {
        /// Underlying client error.
        source: nostr_sdk::client::Error,
    },

    /// A security-sensitive query ended without a complete EOSE response.
    #[error("incomplete Nostr query: {reason}")]
    IncompleteQuery {
        /// Why the query could not establish a complete result.
        reason: &'static str,
    },

    /// A query that should return one event returned none.
    #[error("missing Nostr event for {context}")]
    MissingEvent {
        /// Human-readable query context.
        context: &'static str,
    },
}
