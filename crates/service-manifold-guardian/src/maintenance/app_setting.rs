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
/// Most default mods one federation may list.
pub const FEDERATION_APP_SETTING_MAX_FEDIMODS: usize = 32;

/// Matrix's room-id length limit.
const MATRIX_ROOM_ID_MAX_BYTES: usize = 255;
const FEDIMOD_ID_MAX_BYTES: usize = 64;
const FEDIMOD_TITLE_MAX_BYTES: usize = 64;
const FEDIMOD_COLOR_MAX_BYTES: usize = 32;
const CURRENCY_ALIAS_MAX_BYTES: usize = 32;
const COMMUNITY_INVITE_HRPS: [&str; 2] = ["fedi:community", "fedi:community2"];

/// One Fedi app setting served by FA metadata maintenance.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum FederationAppSetting {
    /// Communities every member joins ("linked communities").
    AutojoinCommunities,
    /// Matrix group chats shown to every member.
    DefaultGroupChats,
    /// Mods shown on the federation home screen.
    FediMods,
    /// Message pinned to the community home screen.
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
    /// Refuses payment requests above this balance.
    MaxBalanceMsats,
    /// Refuses invoices above this amount.
    MaxInvoiceMsats,
    /// Refuses stable balances above this amount.
    MaxStableBalanceMsats,
}

enum ValueKind {
    CommunityInvites,
    MatrixRoomIds,
    FediMods,
    PinnedMessage,
    Currency,
    Flag,
    Msats,
}

impl FederationAppSetting {
    /// Every served setting.
    pub const ALL: [Self; 14] = [
        Self::AutojoinCommunities,
        Self::DefaultGroupChats,
        Self::FediMods,
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
            Self::FediMods => "fedi:fedimods",
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
    /// The caps keep every served field together well under
    /// [`FEDERATION_METADATA_OBJECT_MAX_BYTES`](super::FEDERATION_METADATA_OBJECT_MAX_BYTES),
    /// so filling them cannot wedge later maintenance.
    #[must_use]
    pub const fn raw_max_bytes(self) -> usize {
        match self.kind() {
            ValueKind::CommunityInvites => 16_384,
            ValueKind::MatrixRoomIds => 8_192,
            ValueKind::FediMods => 32_768,
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
            Self::FediMods => ValueKind::FediMods,
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
                let iso = value.len() == 3 && value.bytes().all(|b| b.is_ascii_uppercase());
                let alias = value.bytes().next().is_some_and(|b| b.is_ascii_lowercase())
                    && value.bytes().all(|b| b.is_ascii_lowercase() || b == b'_');
                if iso || alias {
                    Ok(())
                } else {
                    Err(invalid(
                        "must be a three-letter uppercase code or a lowercase currency alias",
                    ))
                }
            }
            ValueKind::PinnedMessage => {
                // The app hides an empty pinned message, so "" is the clear.
                if value.is_empty() {
                    return Ok(());
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
            ValueKind::FediMods => validate_unique_list(
                field,
                value,
                FEDERATION_APP_SETTING_MAX_FEDIMODS,
                |entry: &FediMod| validate_fedimod(entry).map(|()| entry.id.clone()),
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

/// Accept what Fedi's `CommunityInvite::from_str` accepts: a bech32 string,
/// compared case-insensitively, whose HRP is a community HRP and whose
/// payload is a JSON object.
fn validate_community_invite(entry: &str) -> Result<(), &'static str> {
    let (hrp, data) = bech32::decode(&entry.to_lowercase())
        .map_err(|_| "entries must be bech32 community invite codes")?;
    if !COMMUNITY_INVITE_HRPS.contains(&hrp.as_str()) {
        return Err("entries must be community invite codes");
    }
    serde_json::from_slice::<serde_json::Map<String, serde_json::Value>>(&data)
        .map(|_| ())
        .map_err(|_| "entries must carry a community invite payload")
}

/// Accept a Matrix room id in the shape the app requires (`!opaque:server`).
fn validate_matrix_room_id(entry: &str) -> Result<(), &'static str> {
    let shaped = entry.len() <= MATRIX_ROOM_ID_MAX_BYTES
        && !entry
            .chars()
            .any(|ch| ch.is_whitespace() || ch.is_control())
        && entry
            .strip_prefix('!')
            .and_then(|rest| rest.split_once(':'))
            .is_some_and(|(opaque, server)| !opaque.is_empty() && !server.is_empty());
    if shaped {
        Ok(())
    } else {
        Err("entries must be Matrix room ids like `!opaque:server`")
    }
}

/// One mod entry, limited to the fields the app reads.
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct FediMod {
    id: String,
    title: String,
    url: String,
    image_url: Option<String>,
    description: Option<String>,
    color: Option<String>,
}

fn validate_fedimod(entry: &FediMod) -> Result<(), InvalidFederationMetadataValue> {
    const FIELD: &str = "fedi:fedimods";
    let trimmed = validate_trimmed(FIELD, &entry.id, 1, FEDIMOD_ID_MAX_BYTES)?;
    validate_visible(FIELD, trimmed)?;
    let trimmed = validate_trimmed(FIELD, &entry.title, 1, FEDIMOD_TITLE_MAX_BYTES)?;
    validate_visible(FIELD, trimmed)?;
    validate_url(FIELD, &entry.url, FEDERATION_METADATA_ICON_URL_MAX_BYTES)?;
    if let Some(image_url) = &entry.image_url {
        validate_url(FIELD, image_url, FEDERATION_METADATA_ICON_URL_MAX_BYTES)?;
    }
    if let Some(description) = &entry.description {
        let trimmed = validate_trimmed(
            FIELD,
            description,
            1,
            FEDERATION_METADATA_WELCOME_MESSAGE_MAX_BYTES,
        )?;
        validate_visible(FIELD, trimmed)?;
    }
    if let Some(color) = &entry.color {
        validate_trimmed(FIELD, color, 1, FEDIMOD_COLOR_MAX_BYTES)?;
    }
    Ok(())
}
