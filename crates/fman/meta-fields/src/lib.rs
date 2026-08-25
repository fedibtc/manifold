//! The compiled set of consensus-metadata keys this FMan will relay.
//!
//! `SetMetaField` is a request to change what a federation says about itself,
//! so the FMan does not act as a transparent pipe for it: every key must have
//! a typed [`MetaFieldValidator`] compiled into the `VALIDATORS` registry, and
//! an unrecognized key is refused rather than forwarded
//! ([`ARCH-fleet-manager-product-boundary`](../../specs/ARCH-fleet-manager-product-boundary.md), *Complete
//! the declared cross-component surface*). Adding a key here is the decision
//! to serve it; there is no runtime allowlist to widen.
//!
//! The narrow maintenance keys served here mirror Guardianito's existing
//! automatic-approval behavior. `fedi:fman_api_urls`, privacy-policy keys, and
//! the formation-owned seat directory and guardian-fee remittance value are
//! deliberately absent. The guardian-fee rate is the only fee field served
//! here after formation.

use fedi_decentralized_service_fleet_manager::{
    FEDERATION_ICON_URL_META_FIELD_KEY, FEDERATION_METADATA_RAW_MAX_BYTES,
    FEDERATION_NAME_META_FIELD_KEY, FederationMetadataIconUrl, FederationMetadataName,
    FederationMetadataWelcomeMessage, GUARDIAN_FEE_SEND_PPM_META_FIELD_KEY,
    GUARDIANITO_TERMS_OF_SERVICE_URL, MetaFieldKey, MetaFieldValue,
    TERMS_OF_SERVICE_URL_META_FIELD_KEY, WELCOME_MESSAGE_META_FIELD_KEY,
};

/// Bound attacker-controlled keys before either child access or logging.
const META_FIELD_KEY_MAX_BYTES: usize = 128;

/// One consensus-metadata key's compiled validation policy.
///
/// An implementation owns the complete decision for its key: the absolute raw
/// resource bound checked before any child access, and the semantic validation
/// run before a proposal can reach fedimintd.
pub trait MetaFieldValidator {
    /// The exact meta key this validator serves.
    fn key(&self) -> &'static str;

    /// Absolute raw byte cap on the wire value, enforced before child access.
    ///
    /// Guardianito's semantic validators trim some values, but the exact
    /// original string is what guardians place in the whole consensus object.
    /// The absolute raw cap therefore bounds child work and retry waves
    /// without narrowing Guardianito's ordinary padded-value behavior.
    fn raw_value_max_bytes(&self) -> usize;

    /// Validate a proposed value against this key's compiled policy.
    fn validate(&self, value: &str) -> Result<(), MetaFieldError>;
}

/// The complete compiled validator set. Adding an entry here is the decision
/// to serve its key.
const VALIDATORS: &[&dyn MetaFieldValidator] = &[
    &GuardianFeeSendPpmValidator,
    &FederationNameValidator,
    &FederationIconUrlValidator,
    &WelcomeMessageValidator,
    &TermsOfServiceUrlValidator,
];

fn validator_for(key: &str) -> Option<&'static dyn MetaFieldValidator> {
    VALIDATORS
        .iter()
        .copied()
        .find(|validator| validator.key() == key)
}

/// Why a proposed metadata write was refused before reaching fedimintd.
#[derive(Debug, Eq, PartialEq)]
pub enum MetaFieldError {
    /// The key has no compiled validator, so this FMan will not relay it.
    UnknownKey,

    /// The value failed its key's typed validator. The reason is daemon log
    /// material — the wire answers a bare `MetaValueInvalid`.
    InvalidValue(String),
}

/// Validate the wire wrappers through the complete compiled dispatch before
/// probing fedimintd.
pub fn validate_meta_field(
    key: &MetaFieldKey,
    value: &MetaFieldValue,
) -> Result<(), MetaFieldError> {
    if key.0.len() > META_FIELD_KEY_MAX_BYTES {
        return Err(MetaFieldError::UnknownKey);
    }
    let validator = validator_for(&key.0).ok_or(MetaFieldError::UnknownKey)?;
    let max_bytes = validator.raw_value_max_bytes();
    if value.0.len() > max_bytes {
        return invalid(format!(
            "metadata value exceeds its {max_bytes}-byte absolute raw limit"
        ));
    }
    validator.validate(&value.0)
}

fn invalid(reason: impl Into<String>) -> Result<(), MetaFieldError> {
    Err(MetaFieldError::InvalidValue(reason.into()))
}

/// Match Guardianito's current LG-facing federation-name validation by
/// delegating to the shared semantic type the FI pre-validates with.
struct FederationNameValidator;

impl MetaFieldValidator for FederationNameValidator {
    fn key(&self) -> &'static str {
        FEDERATION_NAME_META_FIELD_KEY
    }

    fn raw_value_max_bytes(&self) -> usize {
        FEDERATION_METADATA_RAW_MAX_BYTES
    }

    fn validate(&self, value: &str) -> Result<(), MetaFieldError> {
        FederationMetadataName::try_from(value.to_owned())
            .map(|_| ())
            .map_err(|error| MetaFieldError::InvalidValue(error.to_string()))
    }
}

/// Match Guardianito's current wallet-description validation by delegating to
/// the shared semantic type the FI pre-validates with.
struct WelcomeMessageValidator;

impl MetaFieldValidator for WelcomeMessageValidator {
    fn key(&self) -> &'static str {
        WELCOME_MESSAGE_META_FIELD_KEY
    }

    fn raw_value_max_bytes(&self) -> usize {
        FEDERATION_METADATA_RAW_MAX_BYTES
    }

    fn validate(&self, value: &str) -> Result<(), MetaFieldError> {
        FederationMetadataWelcomeMessage::try_from(value.to_owned())
            .map(|_| ())
            .map_err(|error| MetaFieldError::InvalidValue(error.to_string()))
    }
}

/// Match Guardianito's current URL-only icon behavior (no upload is implied)
/// by delegating to the shared semantic type the FI pre-validates with,
/// including its public-host requirement.
struct FederationIconUrlValidator;

impl MetaFieldValidator for FederationIconUrlValidator {
    fn key(&self) -> &'static str {
        FEDERATION_ICON_URL_META_FIELD_KEY
    }

    fn raw_value_max_bytes(&self) -> usize {
        FEDERATION_METADATA_RAW_MAX_BYTES
    }

    fn validate(&self, value: &str) -> Result<(), MetaFieldError> {
        FederationMetadataIconUrl::try_from(value.to_owned())
            .map(|_| ())
            .map_err(|error| MetaFieldError::InvalidValue(error.to_string()))
    }
}

/// Product/legal direction is still open; MVP deliberately mirrors the only
/// value Guardianito auto-approves instead of inventing a configurable policy.
struct TermsOfServiceUrlValidator;

impl MetaFieldValidator for TermsOfServiceUrlValidator {
    fn key(&self) -> &'static str {
        TERMS_OF_SERVICE_URL_META_FIELD_KEY
    }

    fn raw_value_max_bytes(&self) -> usize {
        GUARDIANITO_TERMS_OF_SERVICE_URL.len()
    }

    fn validate(&self, value: &str) -> Result<(), MetaFieldError> {
        if value == GUARDIANITO_TERMS_OF_SERVICE_URL {
            Ok(())
        } else {
            invalid("terms URL is not the Guardianito-approved fixed value")
        }
    }
}

/// Validate the generic rate-maintenance value. The caller applies its
/// deployment-published floor before child access.
struct GuardianFeeSendPpmValidator;

impl MetaFieldValidator for GuardianFeeSendPpmValidator {
    fn key(&self) -> &'static str {
        GUARDIAN_FEE_SEND_PPM_META_FIELD_KEY
    }

    fn raw_value_max_bytes(&self) -> usize {
        6
    }

    fn validate(&self, value: &str) -> Result<(), MetaFieldError> {
        value
            .parse::<u64>()
            .ok()
            .filter(|value| *value <= 210_000)
            .map(|_| ())
            .ok_or_else(|| MetaFieldError::InvalidValue("invalid guardian-fee rate".to_owned()))
    }
}

#[cfg(test)]
#[path = "../tests/meta_fields.rs"]
mod tests;
