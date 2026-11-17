//! Independently signed, addressable feature policy. Payment fields have no representation here.
use crate::setup_payment_federations::{
    PolicyEventError as Error, SETUP_PAYMENT_FEDERATIONS_MAX_FUTURE_SKEW_SECS,
    authenticate_address_at, is_newer,
};
use fedi_decentralized_domain::{AdmittedFeaturePolicy, FeatureConfigContent, PolicyContentError};
use nostr::{Event, EventBuilder, Filter, Kind, PublicKey, Tag, Timestamp};

/// Addressable kind reserved for complete feature snapshots.
pub const FEATURE_CONFIG_KIND: u16 = 37710;
/// Stable identifier within the feature publisher's namespace.
pub const FEATURE_CONFIG_D_TAG: &str = "manifold-feature-config";
/// Upper bound on feature JSON before deserialization.
pub const FEATURE_CONFIG_MAX_BYTES: usize = 16 * 1024;

/// Select the one configured feature address; relay results remain untrusted.
pub fn feature_config_filter(publisher: PublicKey) -> Filter {
    Filter::new()
        .author(publisher)
        .kind(Kind::Custom(FEATURE_CONFIG_KIND))
        .identifier(FEATURE_CONFIG_D_TAG)
}

fn parse(raw: &str) -> Result<AdmittedFeaturePolicy, PolicyContentError> {
    if raw.len() > FEATURE_CONFIG_MAX_BYTES {
        return Err(PolicyContentError::ContentTooLarge);
    }
    let content: FeatureConfigContent =
        serde_json::from_str(raw).map_err(|_| PolicyContentError::MalformedContent)?;
    content.features.admit()
}

/// Build an unsigned feature publication using consumer-equivalent admission.
pub fn feature_config_event_builder(
    content: &FeatureConfigContent,
) -> Result<EventBuilder, PolicyContentError> {
    let raw = serde_json::to_string(content).map_err(|_| PolicyContentError::MalformedContent)?;
    parse(&raw)?;
    Ok(EventBuilder::new(Kind::Custom(FEATURE_CONFIG_KIND), raw)
        .tag(Tag::identifier(FEATURE_CONFIG_D_TAG)))
}

/// An authenticated complete snapshot and the event from which it was derived.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FeatureConfigEvent {
    event: Event,
    policy: AdmittedFeaturePolicy,
}

impl FeatureConfigEvent {
    /// Revalidate a previously admitted record, without applying the current clock.
    pub fn restore(event: Event, publisher: PublicKey) -> Result<Self, Error> {
        if event.content.len() > FEATURE_CONFIG_MAX_BYTES {
            return Err(Error::Content(PolicyContentError::ContentTooLarge));
        }
        authenticate_address_at(&event, publisher, FEATURE_CONFIG_KIND, FEATURE_CONFIG_D_TAG)?;
        let policy = parse(&event.content).map_err(Error::Content)?;
        Ok(Self { event, policy })
    }
    /// Admit a fetched publication only if its timestamp and replacement order are valid.
    pub fn admit(
        event: Event,
        publisher: PublicKey,
        now: Timestamp,
        current: Option<&Self>,
    ) -> Result<Self, Error> {
        let candidate = Self::restore(event, publisher)?;
        if current.is_some_and(|old| old.event.id == candidate.event.id) {
            return Ok(candidate);
        }
        if candidate.event.created_at.as_secs()
            > now
                .as_secs()
                .saturating_add(SETUP_PAYMENT_FEDERATIONS_MAX_FUTURE_SKEW_SECS)
        {
            return Err(Error::CreatedTooFarInFuture);
        }
        if current.is_some_and(|old| !is_newer(&candidate.event, &old.event)) {
            return Err(Error::Rollback);
        }
        Ok(candidate)
    }
    /// Authenticate a retained floor even when this reader cannot interpret its content,
    /// then select the highest admissible snapshot strictly above that floor.
    pub fn select(
        stored: Option<Event>,
        publisher: PublicKey,
        candidates: impl IntoIterator<Item = Event>,
        now: Timestamp,
    ) -> Result<Option<Self>, Error> {
        let mut best = match stored.as_ref() {
            None => None,
            Some(event) => match Self::restore(event.clone(), publisher) {
                Ok(snapshot) => Some(snapshot),
                Err(Error::Content(_)) => {
                    authenticate_address_at(
                        event,
                        publisher,
                        FEATURE_CONFIG_KIND,
                        FEATURE_CONFIG_D_TAG,
                    )?;
                    None
                }
                Err(err) => return Err(err),
            },
        };
        for candidate in candidates {
            if stored
                .as_ref()
                .is_some_and(|floor| !is_newer(&candidate, floor))
            {
                continue;
            }
            if let Ok(next) = Self::admit(candidate, publisher, now, best.as_ref()) {
                best = Some(next);
            }
        }
        Ok(best)
    }

    /// Return the full signed event to retain for restart.
    pub fn event(&self) -> &Event {
        &self.event
    }
    /// Consume the envelope and return only its authenticated feature policy.
    pub fn into_policy(self) -> AdmittedFeaturePolicy {
        self.policy
    }
}

#[cfg(test)]
mod tests;
