//! Fedi app settings an FA may change after formation.
//!
//! Each [`FederationAppSetting`] names one federation meta key that the Fedi
//! app reads to change behavior, with the value rules that app parses. MG
//! serves exactly these keys through `SetMetaField`; Fedi-operated and
//! payment-routing keys such as `vetted_gateways` are deliberately absent
//! ([`SPEC-fa-metadata-maintenance`](../../../mg/specs/SPEC-fa-metadata-maintenance.md)).
//!
//! The meta module has no delete, so every setting has an explicit "off"
//! value: `false` for flags, `[]` for lists, and `""` for the pinned message.

use std::collections::BTreeSet;

use serde::Deserialize;

use super::{
    FEDERATION_METADATA_ICON_URL_MAX_BYTES, FEDERATION_METADATA_WELCOME_MESSAGE_MAX_BYTES,
    InvalidFederationMetadataValue, validate_trimmed, validate_url, validate_visible,
};

/// Largest msat limit accepted: 21 million BTC.
pub const FEDERATION_APP_SETTING_MAX_MSATS: u64 = 2_100_000_000_000_000_000;
/// Most communities one federation may link.
pub const FEDERATION_APP_SETTING_MAX_COMMUNITIES: usize = 10;
/// Most default group chats one federation may list.
pub const FEDERATION_APP_SETTING_MAX_GROUP_CHATS: usize = 20;

/// Matrix's room-id length limit.
const MATRIX_ROOM_ID_MAX_BYTES: usize = 255;
const CURRENCY_ALIAS_MAX_BYTES: usize = 32;
const COMMUNITY_V1_HRP: &str = "fedi:community";
const COMMUNITY_V2_HRP: &str = "fedi:community2";

/// The Fedi app's `SupportedCurrency` codes. The app formats any other code
/// with a zero exchange rate, so MG accepts only these.
const SUPPORTED_CURRENCY_CODES: [&str; 68] = [
    "USD", "ARS", "AUD", "BDT", "BIF", "BOB", "BRL", "BTN", "BWP", "CAD", "CDF", "CHF", "CLP",
    "CNY", "COP", "CRC", "CUP", "CZK", "DJF", "DOP", "ERN", "ETB", "EUR", "GBP", "GHS", "GTQ",
    "HKD", "HNL", "IDR", "INR", "JPY", "KES", "KHR", "KRW", "LBP", "LKR", "PAB", "MMK", "MWK",
    "MXN", "MYR", "MZN", "NAD", "NGN", "NIO", "NZD", "PEN", "PHP", "PKR", "PLN", "PYG", "RWF",
    "SDG", "SOS", "SRD", "SSP", "THB", "TZS", "UAH", "UGX", "UYU", "VES", "VND", "ZAR", "ZMW",
    "ZWL", "XAF", "XOF",
];
/// The Fedi app's `CurrencyAliases` names. `Intl.NumberFormat` throws on an
/// unknown alias, so MG accepts only these.
const CURRENCY_ALIASES: [&str; 14] = [
    "cameroon",
    "central_african_republic",
    "chad",
    "equatorial_guinea",
    "gabon",
    "republic_of_the_congo",
    "benin",
    "burkina_faso",
    "cote_d_ivoire",
    "guinea_bissau",
    "mali",
    "niger",
    "senegal",
    "togo",
];

/// One Fedi app setting served by FA metadata maintenance.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum FederationAppSetting {
    /// Communities every member joins ("linked communities").
    AutojoinCommunities,
    /// Matrix group chats shown to every member.
    DefaultGroupChats,
    /// Compact message on the native Wallet screen.
    PinnedMessage,
    /// Default display currency.
    DefaultCurrency,
    /// Hides the stability pool unless `false`; the app's default is hidden.
    StabilityPoolDisabled,
    /// Hides multispend unless `false`; the app's default is hidden.
    MultispendDisabled,
    /// Hides the invite code after join.
    InviteCodesDisabled,
    /// Blocks new joins through the Fedi app.
    NewMembersDisabled,
    /// Hides offline ecash QR codes.
    OfflineWalletDisabled,
    /// Hides on-chain deposits.
    OnchainDepositsDisabled,
    /// Disables Receive once the member's balance reaches this amount.
    MaxBalanceMsats,
    /// Largest invoice a member can create; `0` disables Receive.
    MaxInvoiceMsats,
    /// Refuses stable balances above this amount.
    MaxStableBalanceMsats,
}

enum ValueKind {
    CommunityInvites,
    MatrixRoomIds,
    PinnedMessage,
    Currency,
    Flag,
    Msats,
}

impl FederationAppSetting {
    /// Every served setting.
    pub const ALL: [Self; 13] = [
        Self::AutojoinCommunities,
        Self::DefaultGroupChats,
        Self::PinnedMessage,
        Self::DefaultCurrency,
        Self::StabilityPoolDisabled,
        Self::MultispendDisabled,
        Self::InviteCodesDisabled,
        Self::NewMembersDisabled,
        Self::OfflineWalletDisabled,
        Self::OnchainDepositsDisabled,
        Self::MaxBalanceMsats,
        Self::MaxInvoiceMsats,
        Self::MaxStableBalanceMsats,
    ];

    /// The exact meta key. The app prefers `fedi:<field>` over `<field>`, so
    /// these prefixed keys override any unprefixed legacy value.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::AutojoinCommunities => "fedi:autojoin_communities",
            Self::DefaultGroupChats => "fedi:default_group_chats",
            Self::PinnedMessage => "fedi:pinned_message",
            Self::DefaultCurrency => "fedi:default_currency",
            Self::StabilityPoolDisabled => "fedi:stability_pool_disabled",
            Self::MultispendDisabled => "fedi:multispend_disabled",
            Self::InviteCodesDisabled => "fedi:invite_codes_disabled",
            Self::NewMembersDisabled => "fedi:new_members_disabled",
            Self::OfflineWalletDisabled => "fedi:offline_wallet_disabled",
            Self::OnchainDepositsDisabled => "fedi:onchain_deposits_disabled",
            Self::MaxBalanceMsats => "fedi:max_balance_msats",
            Self::MaxInvoiceMsats => "fedi:max_invoice_msats",
            Self::MaxStableBalanceMsats => "fedi:max_stable_balance_msats",
        }
    }

    /// The setting served under `key`, if any.
    #[must_use]
    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|setting| setting.key() == key)
    }

    /// Absolute raw byte cap, checked before parsing.
    ///
    /// Together, the app settings' stored size stays under a quarter of
    /// [`FEDERATION_METADATA_OBJECT_MAX_BYTES`](super::FEDERATION_METADATA_OBJECT_MAX_BYTES).
    /// The complete target is still subject to that aggregate cap.
    #[must_use]
    pub const fn raw_max_bytes(self) -> usize {
        match self.kind() {
            ValueKind::CommunityInvites => 16_384,
            ValueKind::MatrixRoomIds => 8_192,
            ValueKind::PinnedMessage => 2_048,
            ValueKind::Currency => CURRENCY_ALIAS_MAX_BYTES,
            ValueKind::Flag => "false".len(),
            // Digits of FEDERATION_APP_SETTING_MAX_MSATS.
            ValueKind::Msats => 19,
        }
    }

    const fn kind(self) -> ValueKind {
        match self {
            Self::AutojoinCommunities => ValueKind::CommunityInvites,
            Self::DefaultGroupChats => ValueKind::MatrixRoomIds,
            Self::PinnedMessage => ValueKind::PinnedMessage,
            Self::DefaultCurrency => ValueKind::Currency,
            Self::StabilityPoolDisabled
            | Self::MultispendDisabled
            | Self::InviteCodesDisabled
            | Self::NewMembersDisabled
            | Self::OfflineWalletDisabled
            | Self::OnchainDepositsDisabled => ValueKind::Flag,
            Self::MaxBalanceMsats | Self::MaxInvoiceMsats | Self::MaxStableBalanceMsats => {
                ValueKind::Msats
            }
        }
    }

    /// Validate `value` for this setting. The exact string is what guardians
    /// submit; nothing is normalized.
    pub fn validate(self, value: &str) -> Result<(), InvalidFederationMetadataValue> {
        let field = self.key();
        let max_bytes = self.raw_max_bytes();
        if value.len() > max_bytes {
            return Err(InvalidFederationMetadataValue::RawTooLarge { field, max_bytes });
        }
        let invalid = |reason: &str| InvalidFederationMetadataValue::InvalidAppSetting {
            field,
            reason: reason.to_owned(),
        };
        match self.kind() {
            ValueKind::Flag => match value {
                "true" | "false" => Ok(()),
                _ => Err(invalid("must be `true` or `false`")),
            },
            ValueKind::Msats => validate_msats(value).ok_or_else(|| {
                invalid("must be a canonical decimal msat amount up to 21 million BTC")
            }),
            ValueKind::Currency => {
                if SUPPORTED_CURRENCY_CODES.contains(&value) || CURRENCY_ALIASES.contains(&value) {
                    Ok(())
                } else {
                    Err(invalid(
                        "must be a currency code or alias the Fedi app supports",
                    ))
                }
            }
            ValueKind::PinnedMessage => {
                // The app hides an empty pinned message, so "" is the clear.
                if value.is_empty() {
                    return Ok(());
                }
                // Pinned messages reject controls even in surrounding whitespace,
                // which also bounds JSON escaping of the stored value.
                if value.chars().any(char::is_control) {
                    return Err(InvalidFederationMetadataValue::ControlCharacter { field });
                }
                let trimmed = validate_trimmed(
                    field,
                    value,
                    1,
                    FEDERATION_METADATA_WELCOME_MESSAGE_MAX_BYTES,
                )?;
                validate_visible(field, trimmed)
            }
            ValueKind::CommunityInvites => validate_unique_list(
                field,
                value,
                FEDERATION_APP_SETTING_MAX_COMMUNITIES,
                |entry: &String| {
                    validate_community_invite(entry)
                        .map(|()| entry.to_lowercase())
                        .map_err(&invalid)
                },
            ),
            ValueKind::MatrixRoomIds => validate_unique_list(
                field,
                value,
                FEDERATION_APP_SETTING_MAX_GROUP_CHATS,
                |entry: &String| {
                    validate_matrix_room_id(entry)
                        .map(|()| entry.clone())
                        .map_err(&invalid)
                },
            ),
        }
    }
}

/// A validated [`FederationAppSetting`] value.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FederationAppSettingValue {
    setting: FederationAppSetting,
    value: String,
}

impl FederationAppSettingValue {
    /// Validate `value` for `setting`.
    pub fn new(
        setting: FederationAppSetting,
        value: impl Into<String>,
    ) -> Result<Self, InvalidFederationMetadataValue> {
        let value = value.into();
        setting.validate(&value)?;
        Ok(Self { setting, value })
    }

    /// The setting this value is for.
    #[must_use]
    pub fn setting(&self) -> FederationAppSetting {
        self.setting
    }

    /// The exact value submitted to consensus.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.value
    }

    pub(super) fn into_inner(self) -> String {
        self.value
    }
}

fn validate_msats(value: &str) -> Option<()> {
    let canonical = !value.is_empty()
        && value.bytes().all(|b| b.is_ascii_digit())
        && (value == "0" || !value.starts_with('0'));
    let amount = value.parse::<u64>().ok().filter(|_| canonical)?;
    (amount <= FEDERATION_APP_SETTING_MAX_MSATS).then_some(())
}

/// Parse a JSON array whose entries each validate to a distinct identity.
fn validate_unique_list<T, F>(
    field: &'static str,
    value: &str,
    max_entries: usize,
    mut identity: F,
) -> Result<(), InvalidFederationMetadataValue>
where
    T: for<'de> Deserialize<'de>,
    F: FnMut(&T) -> Result<String, InvalidFederationMetadataValue>,
{
    let invalid = |reason: &str| InvalidFederationMetadataValue::InvalidAppSetting {
        field,
        reason: reason.to_owned(),
    };
    let entries: Vec<T> = serde_json::from_str(value)
        .map_err(|_| invalid("must be a JSON array of the documented entries"))?;
    if entries.len() > max_entries {
        return Err(invalid(&format!("must list at most {max_entries} entries")));
    }
    let mut seen = BTreeSet::new();
    for entry in &entries {
        if !seen.insert(identity(entry)?) {
            return Err(invalid("must not repeat an entry"));
        }
    }
    Ok(())
}

/// Fedi's `CommunityInviteV1` payload.
#[derive(Deserialize)]
struct CommunityInviteV1 {
    community_meta_url: String,
}

/// Fedi's `CommunityInviteV2` payload.
#[derive(Deserialize)]
struct CommunityInviteV2 {
    #[allow(dead_code)]
    author_pubkey: nostr::PublicKey,
    #[allow(dead_code)]
    community_uuid_hex: String,
    decryption_key: String,
}

/// Accept what Fedi's `CommunityInvite::from_str` accepts: the lowercased
/// string decodes as bech32 with the V2 HRP and a V2 payload, or with the V1
/// HRP and a V1 payload. V1 metadata URLs also follow the icon URL rules,
/// because members' devices fetch them automatically.
fn validate_community_invite(entry: &str) -> Result<(), &'static str> {
    use base64::Engine as _;
    use base64::alphabet;
    use base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig};

    let (hrp, data) = bech32::decode(&entry.to_lowercase())
        .map_err(|_| "entries must be bech32 community invite codes")?;
    let payload = "entries must carry a valid community invite payload";
    match hrp.as_str() {
        COMMUNITY_V2_HRP => {
            let invite: CommunityInviteV2 = serde_json::from_slice(&data).map_err(|_| payload)?;
            // Fedi decodes the key with serde_with's standard base64, which
            // ignores padding.
            let engine = GeneralPurpose::new(
                &alphabet::STANDARD,
                GeneralPurposeConfig::new()
                    .with_decode_padding_mode(DecodePaddingMode::Indifferent),
            );
            let key = engine.decode(&invite.decryption_key).map_err(|_| payload)?;
            if key.len() != 32 {
                return Err(payload);
            }
            Ok(())
        }
        COMMUNITY_V1_HRP => {
            let invite: CommunityInviteV1 = serde_json::from_slice(&data).map_err(|_| payload)?;
            validate_url(
                "community metadata URL",
                &invite.community_meta_url,
                FEDERATION_METADATA_ICON_URL_MAX_BYTES,
            )
            .map_err(|_| "community metadata URLs must be public HTTP(S) URLs")
        }
        _ => Err("entries must be community invite codes"),
    }
}

/// Accept a Matrix room id as Fedi's Ruma parses it: `!` then a nonempty
/// body, at most 255 bytes. Legacy ids carry `:server`; room version 12 ids
/// are a bare hash.
fn validate_matrix_room_id(entry: &str) -> Result<(), &'static str> {
    let shaped = entry.len() <= MATRIX_ROOM_ID_MAX_BYTES
        && !entry
            .chars()
            .any(|ch| ch.is_whitespace() || ch.is_control())
        && entry.strip_prefix('!').is_some_and(|rest| !rest.is_empty());
    if shaped {
        Ok(())
    } else {
        Err("entries must be Matrix room ids starting with `!`")
    }
}
