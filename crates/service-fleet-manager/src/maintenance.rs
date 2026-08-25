//! Semantic values for FI-owned post-formation metadata maintenance.
//!
//! These types are shared by `fi-client` and FMan so the caller can reject an
//! invalid update before lease, signing, or network work while every FMan still
//! applies the same compiled policy at the authorization boundary.

use std::fmt;

use url::Url;

use crate::{
    FEDERATION_ICON_URL_META_FIELD_KEY, FEDERATION_NAME_META_FIELD_KEY, MetaFieldKey,
    MetaFieldValue, TERMS_OF_SERVICE_URL_META_FIELD_KEY, WELCOME_MESSAGE_META_FIELD_KEY,
};

/// Guardianito's post-trim display-name limit.
pub const FEDERATION_METADATA_NAME_MAX_BYTES: usize = 30;
/// Guardianito's post-trim welcome-message limit.
pub const FEDERATION_METADATA_WELCOME_MESSAGE_MAX_BYTES: usize = 500;
/// Guardianito's post-trim icon-URL limit.
pub const FEDERATION_METADATA_ICON_URL_MAX_BYTES: usize = 2_048;
/// Absolute authenticated-request resource bound for trim-valid metadata.
///
/// Guardianito preserves the original value after validating the trimmed
/// value. This bound is deliberately larger than every semantic maximum so it
/// bounds signing and fan-out without narrowing ordinary padded inputs.
pub const FEDERATION_METADATA_RAW_MAX_BYTES: usize = 65_536;
/// Maximum raw size of the complete consensus metadata object accepted by FI
/// maintenance and FMan mutation paths.
///
/// Fedimint's upstream `MetaValue` permits much larger values. This local cap
/// bounds pre-hash, parse, clone, canonicalization, and guardian fan-out work
/// while leaving room for every currently supported bounded field.
pub const FEDERATION_METADATA_OBJECT_MAX_BYTES: usize = 1_048_576;
/// The only terms document Guardianito currently auto-approves.
pub const GUARDIANITO_TERMS_OF_SERVICE_URL: &str = "https://public.qgcut.org/OG_Federation_ToS.pdf";

/// Why a typed federation-metadata value could not be constructed.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum InvalidFederationMetadataValue {
    /// The original value exceeds the pre-network resource ceiling.
    #[error("{field} exceeds the {max_bytes}-byte absolute raw limit")]
    RawTooLarge {
        /// Human-readable metadata field.
        field: &'static str,
        /// Inclusive maximum original-string size.
        max_bytes: usize,
    },
    /// The trimmed value is outside Guardianito's accepted size range.
    #[error("{field} must be {min_bytes} through {max_bytes} bytes after trimming")]
    InvalidTrimmedLength {
        /// Human-readable metadata field.
        field: &'static str,
        /// Inclusive trimmed minimum.
        min_bytes: usize,
        /// Inclusive trimmed maximum.
        max_bytes: usize,
    },
    /// The value contains a character Guardianito refuses.
    #[error("{field} contains a control character")]
    ControlCharacter {
        /// Human-readable metadata field.
        field: &'static str,
    },
    /// The value contains a character that renders invisibly or reorders the
    /// text around it, so the bytes and the glyphs disagree about what the
    /// federation says.
    #[error("{field} contains {class}")]
    InvisibleCharacter {
        /// Human-readable metadata field.
        field: &'static str,
        /// The refused character class.
        class: &'static str,
    },
    /// The display name contains Guardianito's refused phrase.
    #[error("federation name contains a refused phrase")]
    RefusedNamePhrase,
    /// The icon is not an HTTP(S) URL.
    #[error("federation icon URL must be an HTTP(S) URL")]
    InvalidIconUrl,
    /// The icon URL names a host that is not publicly resolvable, which would
    /// turn every wallet rendering the icon into an SSRF/probing client.
    #[error("federation icon URL {reason}")]
    NonPublicIconHost {
        /// Why the host cannot be fetched from the public internet.
        reason: &'static str,
    },
}

macro_rules! semantic_string {
    ($(#[$metadata:meta])* $name:ident) => {
        $(#[$metadata])*
        #[derive(Clone, Debug, Eq, PartialEq)]
        pub struct $name(String);

        impl $name {
            /// Borrow the original, byte-preserved metadata value.
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }

            fn into_inner(self) -> String {
                self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(&self.0)
            }
        }
    };
}

semantic_string!(
    /// A Guardianito-compatible federation display name.
    ///
    /// The original UTF-8 string is limited to 65,536 bytes. Validation trims
    /// surrounding whitespace, then requires 3 through 30 bytes, rejects NUL,
    /// control, bidirectional-control, and zero-width characters, and refuses
    /// the phrase “payment request rejected” case-insensitively. The original
    /// untrimmed bytes are retained and submitted to federation consensus.
    FederationMetadataName
);
semantic_string!(
    /// A Guardianito-compatible federation icon URL.
    ///
    /// The original UTF-8 string is limited to 65,536 bytes. Validation trims
    /// surrounding whitespace, then requires 1 through 2,048 bytes, rejects
    /// NUL and control characters, and requires an HTTP(S) URL whose host is
    /// publicly resolvable ([`validate_public_icon_host`]). The original
    /// untrimmed bytes are retained and submitted to federation consensus.
    FederationMetadataIconUrl
);
semantic_string!(
    /// A Guardianito-compatible welcome message used by Fedi as description.
    ///
    /// The original UTF-8 string is limited to 65,536 bytes. Validation trims
    /// surrounding whitespace, then requires 1 through 500 bytes and rejects
    /// NUL, control, bidirectional-control, and zero-width characters. The
    /// original untrimmed bytes are retained and submitted to federation
    /// consensus.
    FederationMetadataWelcomeMessage
);

impl TryFrom<String> for FederationMetadataName {
    type Error = InvalidFederationMetadataValue;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        let trimmed = validate_trimmed(
            "federation name",
            &value,
            3,
            FEDERATION_METADATA_NAME_MAX_BYTES,
        )?;
        validate_visible("federation name", trimmed)?;
        if trimmed.to_lowercase().contains("payment request rejected") {
            return Err(InvalidFederationMetadataValue::RefusedNamePhrase);
        }
        Ok(Self(value))
    }
}

impl TryFrom<String> for FederationMetadataIconUrl {
    type Error = InvalidFederationMetadataValue;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        let trimmed = validate_trimmed(
            "federation icon URL",
            &value,
            1,
            FEDERATION_METADATA_ICON_URL_MAX_BYTES,
        )?;
        match Url::parse(trimmed) {
            Ok(url) if matches!(url.scheme(), "http" | "https") => {
                validate_public_icon_host(&url)?;
                Ok(Self(value))
            }
            _ => Err(InvalidFederationMetadataValue::InvalidIconUrl),
        }
    }
}

impl TryFrom<String> for FederationMetadataWelcomeMessage {
    type Error = InvalidFederationMetadataValue;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        let trimmed = validate_trimmed(
            "welcome message",
            &value,
            1,
            FEDERATION_METADATA_WELCOME_MESSAGE_MAX_BYTES,
        )?;
        validate_visible("welcome message", trimmed)?;
        Ok(Self(value))
    }
}

/// One post-formation metadata mutation supported by Manifold MVP.
///
/// Construction of each value-bearing variant applies the exact shared
/// Guardianito-compatible semantic and raw-resource rules. The FMan repeats
/// the same validation before casting its guardian vote.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FederationMetadataUpdate {
    /// Change the federation display name.
    Name(FederationMetadataName),
    /// Change the federation icon by HTTP(S) URL.
    IconUrl(FederationMetadataIconUrl),
    /// Change the welcome message, reused by Fedi as the description.
    WelcomeMessage(FederationMetadataWelcomeMessage),
    /// Install Guardianito's fixed, automatically approved terms document.
    TermsOfService,
}

impl FederationMetadataUpdate {
    /// Construct a validated [`FederationMetadataName`] mutation.
    pub fn name(value: impl Into<String>) -> Result<Self, InvalidFederationMetadataValue> {
        Ok(Self::Name(FederationMetadataName::try_from(value.into())?))
    }

    /// Construct a validated [`FederationMetadataIconUrl`] mutation.
    pub fn icon_url(value: impl Into<String>) -> Result<Self, InvalidFederationMetadataValue> {
        Ok(Self::IconUrl(FederationMetadataIconUrl::try_from(
            value.into(),
        )?))
    }

    /// Construct a validated [`FederationMetadataWelcomeMessage`] mutation.
    pub fn welcome_message(
        value: impl Into<String>,
    ) -> Result<Self, InvalidFederationMetadataValue> {
        Ok(Self::WelcomeMessage(
            FederationMetadataWelcomeMessage::try_from(value.into())?,
        ))
    }

    /// Return the exact protocol field selected by this typed mutation.
    #[must_use]
    pub fn into_field(self) -> (MetaFieldKey, MetaFieldValue) {
        match self {
            Self::Name(value) => (
                MetaFieldKey(FEDERATION_NAME_META_FIELD_KEY.to_owned()),
                MetaFieldValue(value.into_inner()),
            ),
            Self::IconUrl(value) => (
                MetaFieldKey(FEDERATION_ICON_URL_META_FIELD_KEY.to_owned()),
                MetaFieldValue(value.into_inner()),
            ),
            Self::WelcomeMessage(value) => (
                MetaFieldKey(WELCOME_MESSAGE_META_FIELD_KEY.to_owned()),
                MetaFieldValue(value.into_inner()),
            ),
            Self::TermsOfService => (
                MetaFieldKey(TERMS_OF_SERVICE_URL_META_FIELD_KEY.to_owned()),
                MetaFieldValue(GUARDIANITO_TERMS_OF_SERVICE_URL.to_owned()),
            ),
        }
    }
}

fn validate_raw(field: &'static str, value: &str) -> Result<(), InvalidFederationMetadataValue> {
    if value.len() > FEDERATION_METADATA_RAW_MAX_BYTES {
        return Err(InvalidFederationMetadataValue::RawTooLarge {
            field,
            max_bytes: FEDERATION_METADATA_RAW_MAX_BYTES,
        });
    }
    Ok(())
}

fn validate_trimmed<'value>(
    field: &'static str,
    value: &'value str,
    min_bytes: usize,
    max_bytes: usize,
) -> Result<&'value str, InvalidFederationMetadataValue> {
    validate_raw(field, value)?;
    let trimmed = value.trim();
    if !(min_bytes..=max_bytes).contains(&trimmed.len()) {
        return Err(InvalidFederationMetadataValue::InvalidTrimmedLength {
            field,
            min_bytes,
            max_bytes,
        });
    }
    if trimmed.contains('\0') || trimmed.chars().any(char::is_control) {
        return Err(InvalidFederationMetadataValue::ControlCharacter { field });
    }
    Ok(trimmed)
}

/// Refuse characters that render invisibly or reorder what surrounds them.
fn validate_visible(
    field: &'static str,
    trimmed: &str,
) -> Result<(), InvalidFederationMetadataValue> {
    if let Some(class) = trimmed.chars().find_map(refused_invisible_char) {
        return Err(InvalidFederationMetadataValue::InvisibleCharacter { field, class });
    }
    Ok(())
}

/// Classify characters that render invisibly or reorder what surrounds them.
///
/// `char::is_control` covers only Cc; these are Cf (format) characters, which
/// display code cannot be trusted to surface. The bidirectional set is exactly
/// Unicode's `Bidi_Control` property; the zero-width set covers the invisible
/// joiners and the BOM. A full `Cf`-category check would need a Unicode-table
/// dependency, so the classes are enumerated explicitly instead.
fn refused_invisible_char(ch: char) -> Option<&'static str> {
    match ch {
        '\u{061C}'
        | '\u{200E}'
        | '\u{200F}'
        | '\u{202A}'..='\u{202E}'
        | '\u{2066}'..='\u{2069}' => Some("a bidirectional control character"),
        '\u{200B}'..='\u{200D}' | '\u{FEFF}' => Some("a zero-width character"),
        _ => None,
    }
}

/// Require the icon URL host to name something publicly resolvable.
///
/// Wallets fetch this URL, so a host inside a viewer's own network turns the
/// federation icon into an SSRF/probing vector against whoever renders it.
/// Rejected here: loopback, link-local, and RFC-1918 addresses (including
/// their IPv4-mapped IPv6 spellings), `localhost` and its subdomains, and
/// bare undotted hostnames, which only resolve inside some local network.
fn validate_public_icon_host(url: &Url) -> Result<(), InvalidFederationMetadataValue> {
    let non_public = |reason| InvalidFederationMetadataValue::NonPublicIconHost { reason };
    match url.host() {
        Some(url::Host::Ipv4(ip)) => validate_public_icon_ipv4(ip),
        Some(url::Host::Ipv6(ip)) => {
            if let Some(mapped) = ip.to_ipv4_mapped() {
                return validate_public_icon_ipv4(mapped);
            }
            if ip.is_loopback() {
                return Err(non_public("host is a loopback IPv6 address"));
            }
            if (ip.segments()[0] & 0xffc0) == 0xfe80 {
                return Err(non_public("host is a link-local IPv6 address"));
            }
            Ok(())
        }
        Some(url::Host::Domain(domain)) => {
            let domain = domain.strip_suffix('.').unwrap_or(domain);
            if domain.eq_ignore_ascii_case("localhost")
                || domain.to_ascii_lowercase().ends_with(".localhost")
            {
                return Err(non_public("host is a loopback name"));
            }
            if !domain.contains('.') {
                return Err(non_public("host is not a public domain name"));
            }
            Ok(())
        }
        None => Err(non_public("has no host")),
    }
}

fn validate_public_icon_ipv4(ip: std::net::Ipv4Addr) -> Result<(), InvalidFederationMetadataValue> {
    let non_public = |reason| InvalidFederationMetadataValue::NonPublicIconHost { reason };
    if ip.is_loopback() {
        return Err(non_public("host is a loopback IPv4 address"));
    }
    if ip.is_link_local() {
        return Err(non_public("host is a link-local IPv4 address"));
    }
    if ip.is_private() {
        return Err(non_public("host is a private IPv4 address"));
    }
    Ok(())
}

#[cfg(test)]
#[path = "maintenance/tests.rs"]
mod tests;
