//! Nostr publication and complete admission for the common setup-payment federation set.
//!
//! Contract: `specs/SPEC-setup-payment-federations.md`.

#[cfg(test)]
mod tests;

use fedi_decentralized_domain::{
    AdmittedSetupPaymentFederations, SETUP_PAYMENT_FEDERATIONS_MAX_CONTENT_BYTES,
    SetupPaymentFederationsContent, SetupPaymentFederationsContentError,
};
use nostr::{Event, EventBuilder, Kind, PublicKey, Tag, TagKind, Timestamp};

/// Provisional addressable-event kind for the common setup-payment federation set.
pub const SETUP_PAYMENT_FEDERATIONS_EVENT_KIND: u16 = 37707;

/// Stable `d` tag value for the common setup-payment federation set.
pub const SETUP_PAYMENT_FEDERATIONS_D_TAG: &str = "setup-payment-federations";

/// Maximum amount by which an event timestamp may lead the consumer's clock.
pub const SETUP_PAYMENT_FEDERATIONS_MAX_FUTURE_SKEW_SECS: u64 = 24 * 60 * 60;

/// Serialize and semantically validate policy content, then construct its
/// canonical addressable-event envelope.
///
/// Producer tooling takes the complete shared wire type rather than mirroring
/// its fields as command-line arguments. A future field added to that type is
/// therefore serialized here without a second producer-specific field list.
///
/// # Errors
///
/// Returns an error when serialization fails or the complete content does not
/// pass the same semantic admission used by consumers.
pub fn setup_payment_federations_event_builder(
    content: &SetupPaymentFederationsContent,
) -> Result<EventBuilder, SetupPaymentFederationsEventBuilderError> {
    let content = serde_json::to_string(content)
        .map_err(SetupPaymentFederationsEventBuilderError::Serialize)?;
    AdmittedSetupPaymentFederations::parse(content.as_bytes())
        .map_err(SetupPaymentFederationsEventBuilderError::Content)?;
    Ok(
        EventBuilder::new(Kind::from(SETUP_PAYMENT_FEDERATIONS_EVENT_KIND), content)
            .tag(Tag::identifier(SETUP_PAYMENT_FEDERATIONS_D_TAG)),
    )
}

/// Failure while constructing setup-payment federation event content.
#[derive(Debug)]
pub enum SetupPaymentFederationsEventBuilderError {
    /// The typed wire content could not be serialized.
    Serialize(serde_json::Error),

    /// The serialized content failed consumer-equivalent semantic admission.
    Content(SetupPaymentFederationsContentError),
}

impl core::fmt::Display for SetupPaymentFederationsEventBuilderError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Serialize(error) => write!(formatter, "serialize setup-payment content: {error}"),
            Self::Content(error) => write!(formatter, "invalid setup-payment content: {error}"),
        }
    }
}

impl std::error::Error for SetupPaymentFederationsEventBuilderError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Serialize(error) => Some(error),
            Self::Content(error) => Some(error),
        }
    }
}

/// A fully admitted signed publication and its semantically validated set.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdmittedSetupPaymentFederationsEvent {
    /// Complete signed event retained for rollback protection and offline reuse.
    event: Event,

    /// Parsed invites with canonical federation IDs derived during admission.
    set: AdmittedSetupPaymentFederations,
}

/// A signed event authenticated only as the setup-payment address authority.
///
/// Content remains opaque so an older publisher binary cannot overwrite a
/// newer addressable event merely because its schema has evolved.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuthenticatedSetupPaymentFederationsAddressEvent {
    event: Event,
}

impl AuthenticatedSetupPaymentFederationsAddressEvent {
    /// Return the complete authenticated event.
    #[must_use]
    pub fn event(&self) -> &Event {
        &self.event
    }

    /// Return whether this event wins NIP-01 addressable-event replacement
    /// ordering over another authenticated event.
    #[must_use]
    pub fn is_newer_than(&self, current: &Self) -> bool {
        is_newer(&self.event, &current.event)
    }
}

impl AdmittedSetupPaymentFederationsEvent {
    /// Return the complete signed event that must be persisted atomically.
    #[must_use]
    pub fn event(&self) -> &Event {
        &self.event
    }

    /// Return the semantically admitted common federation set.
    #[must_use]
    pub fn set(&self) -> &AdmittedSetupPaymentFederations {
        &self.set
    }
}

/// Fully authenticate and admit a setup-payment federation publication.
///
/// `current` is the last durably admitted value, if any. A candidate older under
/// NIP-01 addressable-event replacement ordering is rejected. Re-admitting the
/// same event is idempotent even after the local clock moves backwards.
///
/// # Errors
///
/// Returns an error when the event signature, publisher, kind, `d` tag,
/// timestamp, replacement order, or content is invalid.
pub fn admit_setup_payment_federations_event(
    event: &Event,
    pinned_publisher: PublicKey,
    now: Timestamp,
    current: Option<&AdmittedSetupPaymentFederationsEvent>,
) -> Result<AdmittedSetupPaymentFederationsEvent, SetupPaymentFederationsEventError> {
    let set = statically_admit(event, pinned_publisher)?;
    if current.is_some_and(|current| event.id == current.event.id) {
        return Ok(AdmittedSetupPaymentFederationsEvent {
            event: event.clone(),
            set,
        });
    }
    if event.created_at.as_secs()
        > now
            .as_secs()
            .saturating_add(SETUP_PAYMENT_FEDERATIONS_MAX_FUTURE_SKEW_SECS)
    {
        return Err(SetupPaymentFederationsEventError::CreatedTooFarInFuture);
    }
    if current.is_some_and(|current| !is_newer(event, &current.event)) {
        return Err(SetupPaymentFederationsEventError::Rollback);
    }

    Ok(AdmittedSetupPaymentFederationsEvent {
        event: event.clone(),
        set,
    })
}

/// Restore one complete event that the caller previously persisted after admission.
///
/// This repeats signature, authority, `d` tag, and content validation, but
/// deliberately performs no clock or replacement-order check. Callers must use
/// this only for their trusted atomic last-known-good record, never for a newly
/// fetched network candidate.
///
/// # Errors
///
/// Returns an error when the persisted event fails static authentication or
/// content admission.
pub fn restore_durably_admitted_setup_payment_federations_event(
    event: &Event,
    pinned_publisher: PublicKey,
) -> Result<AdmittedSetupPaymentFederationsEvent, SetupPaymentFederationsEventError> {
    let set = statically_admit(event, pinned_publisher)?;
    Ok(AdmittedSetupPaymentFederationsEvent {
        event: event.clone(),
        set,
    })
}

/// Authenticate an event as belonging to the setup-payment address while
/// deliberately treating its content and timestamp as opaque.
///
/// This is the publisher high-water primitive. It prevents an older binary
/// from overwriting a newer signed event whose schema it cannot parse, and it
/// recognizes a signed future-timestamp event before consumers' skew window
/// catches up. Transport callers must bound the complete event first.
///
/// # Errors
///
/// Returns an error when the event signature, publisher, kind, or exact `d`
/// tag is invalid.
pub fn authenticate_setup_payment_federations_address_event(
    event: &Event,
    pinned_publisher: PublicKey,
) -> Result<AuthenticatedSetupPaymentFederationsAddressEvent, SetupPaymentFederationsEventError> {
    authenticate_address(event, pinned_publisher)?;
    Ok(AuthenticatedSetupPaymentFederationsAddressEvent {
        event: event.clone(),
    })
}

fn statically_admit(
    event: &Event,
    pinned_publisher: PublicKey,
) -> Result<AdmittedSetupPaymentFederations, SetupPaymentFederationsEventError> {
    if event.content.len() > SETUP_PAYMENT_FEDERATIONS_MAX_CONTENT_BYTES {
        return Err(SetupPaymentFederationsEventError::Content(
            SetupPaymentFederationsContentError::ContentTooLarge,
        ));
    }
    authenticate_address(event, pinned_publisher)?;
    AdmittedSetupPaymentFederations::parse(event.content.as_bytes())
        .map_err(SetupPaymentFederationsEventError::Content)
}

fn authenticate_address(
    event: &Event,
    pinned_publisher: PublicKey,
) -> Result<(), SetupPaymentFederationsEventError> {
    event
        .verify()
        .map_err(|_| SetupPaymentFederationsEventError::InvalidEvent)?;
    if event.pubkey != pinned_publisher {
        return Err(SetupPaymentFederationsEventError::WrongPublisher);
    }
    if event.kind != Kind::from(SETUP_PAYMENT_FEDERATIONS_EVENT_KIND) {
        return Err(SetupPaymentFederationsEventError::WrongKind);
    }
    if !has_exact_d_tag(event) {
        return Err(SetupPaymentFederationsEventError::WrongDTag);
    }
    Ok(())
}

fn has_exact_d_tag(event: &Event) -> bool {
    let mut d_tags = event
        .tags
        .as_slice()
        .iter()
        .filter(|tag| tag.kind() == TagKind::d());
    let Some(d_tag) = d_tags.next() else {
        return false;
    };
    let d_tag = d_tag.as_slice();
    d_tag.len() == 2
        && d_tag[0] == "d"
        && d_tag[1] == SETUP_PAYMENT_FEDERATIONS_D_TAG
        && d_tags.next().is_none()
}

fn is_newer(candidate: &Event, current: &Event) -> bool {
    candidate.created_at > current.created_at
        || (candidate.created_at == current.created_at && candidate.id < current.id)
}

/// Failure while authenticating or admitting a setup-payment federation event.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SetupPaymentFederationsEventError {
    /// The Nostr event ID or signature is invalid.
    InvalidEvent,

    /// The event author is not the deployment-pinned Fedi publisher.
    WrongPublisher,

    /// The event kind is not the setup-payment federation kind.
    WrongKind,

    /// The event does not contain exactly one canonical setup-payment `d` tag.
    WrongDTag,

    /// The event timestamp is more than 24 hours ahead of the consumer clock.
    CreatedTooFarInFuture,

    /// The event is older than the durably retained publication.
    Rollback,

    /// The signed event content failed structural or semantic admission.
    Content(SetupPaymentFederationsContentError),
}

impl core::fmt::Display for SetupPaymentFederationsEventError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::InvalidEvent => formatter.write_str("invalid Nostr event"),
            Self::WrongPublisher => formatter.write_str("unexpected setup-payment publisher"),
            Self::WrongKind => formatter.write_str("unexpected setup-payment event kind"),
            Self::WrongDTag => formatter.write_str("unexpected setup-payment event d tag"),
            Self::CreatedTooFarInFuture => {
                formatter.write_str("setup-payment event timestamp is too far in the future")
            }
            Self::Rollback => {
                formatter.write_str("setup-payment event would roll back current state")
            }
            Self::Content(error) => {
                write!(formatter, "invalid setup-payment event content: {error}")
            }
        }
    }
}

impl std::error::Error for SetupPaymentFederationsEventError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Content(error) => Some(error),
            _ => None,
        }
    }
}
