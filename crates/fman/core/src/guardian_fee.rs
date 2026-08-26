//! Guardian fees: the recipient side of federation usage fees.
//!
//! Payers charge a usage fee set by federation metadata and remit each
//! guardian's share into the stability-pool BTC balance of an account that
//! guardian controls ([REQ-guardian-fee-remittance](../../../../specs/REQ-guardian-fee-remittance.md)).
//! This module owns the half that decides *where the money goes*: the
//! account derived from the FMan's own identity, and the policy value the
//! seat votes for. Getting either wrong pays a stranger, silently, with no
//! way to notice from inside this daemon — so it sits in core, next to the
//! identity it derives from and the claims that argue about it.
//!
//! Moving the money afterwards is a different problem: it needs a Fedimint
//! client in the guarded federation, it fails loudly, and a failure leaves
//! the funds exactly where they were. That half is [`GuardianFeeVault`], a
//! hole this crate does not fill — `fman-fedimint` does.
//!
//! Three properties are load-bearing and constrain the shape here:
//!
//! - **Fee value movement never uses guardian authority.** The vault runs
//!   through the public client API of a federation this FMan happens to also
//!   guard, and its transactions use only client/module/account authority.
//!   Obtaining the public invite may perform an authenticated read-only phase
//!   probe, while status and backup paths may read public consensus state or
//!   existing guardian files; those reads do not authorize a vault
//!   transaction.
//! - **Nothing new enters operator backup.** The recipient account key is
//!   derived from the FMan mnemonic and the seat id, so the account — and the
//!   funds in it — are recoverable from the mnemonic and the data root alone.
//!   Client databases behind the vault are a cache, not backup material.
//! - **The account exists before the federation does.** It is deliberately
//!   independent of the federation id and of the stability-pool module's own
//!   derivation, so an FMan can commit to where it will be paid before the
//!   DKG ceremony it is committing to
//!   ([SPEC-guardian-fee-policy](../../specs/SPEC-guardian-fee-policy.md)).

use std::collections::BTreeMap;
use std::ops::Range;

use bitcoin::secp256k1;
use fedi_decentralized_service_fleet_manager::{
    FI_GUARDIAN_FEE_WEIGHT, GUARDIAN_GUARDIAN_FEE_WEIGHT, GuardianFeeAccount, GuardianFeeRecipient,
    MAX_GUARDIAN_FEE_RECIPIENTS, SeatId, canonical_guardian_fee_recipient_list,
};
use fedimint_core::Amount;
use fedimint_core::config::FederationId;
use fedimint_core::invite_code::InviteCode;
/// Re-exported so callers name this FMan's remittance account by type rather
/// than by rendered string, without themselves depending on the
/// stability-pool crate.
pub use stability_pool_client::common::AccountId;
use stability_pool_client::common::{
    Account, AccountHistoryItem, AccountHistoryItemKind, AccountType,
};

use crate::remittance_metadata::RemittanceMetadata;

/// One seat's remittance account key: the secret behind the `BtcDepositor`
/// account payers deposit that seat's share into.
///
/// Derived by the caller from the mnemonic and the seat id
/// ([`RootMnemonic::derive_guardian_fee_account_key`](crate::identity::RootMnemonic::derive_guardian_fee_account_key)),
/// which is what makes [`Self::account`] answerable before the federation
/// exists. The stability-pool module would otherwise derive a `BtcDepositor`
/// key of its own from the client root, which depends on the module instance
/// id assigned during config gen; the client is built with this key instead,
/// and operations that move money re-check that before signing.
#[derive(Clone)]
pub struct GuardianFeeAccountKey(secp256k1::Keypair);

impl GuardianFeeAccountKey {
    /// Build the key from derived secret bytes, keeping callers off this
    /// crate's `secp256k1` version.
    ///
    /// Invalid with probability ~2^-128, treated as unreachable exactly like
    /// the other mnemonic-derived keys
    /// ([ARCH-fleet-manager-identity](../../specs/ARCH-fleet-manager-identity.md)).
    pub fn from_secret_bytes(bytes: &[u8; 32]) -> Self {
        Self(
            secp256k1::Keypair::from_seckey_slice(secp256k1::SECP256K1, bytes)
                .expect("HKDF output is a valid secp256k1 scalar"),
        )
    }

    /// The account payers should remit this seat's share to. Needs no
    /// federation, no network, and no DKG.
    pub fn account(&self) -> Account {
        Account::single(self.0.public_key(), AccountType::BtcDepositor)
    }

    /// The signing key itself, for the [`GuardianFeeVault`] implementation:
    /// its stability-pool client must be built holding this key, or the
    /// module signs for an account no payer was told about.
    ///
    /// The confinement that matters is on the way in, not out — the only
    /// constructor takes derived secret bytes, so a key is always one this
    /// FMan's mnemonic and a seat id produced, never an arbitrary keypair
    /// an implementation chose.
    pub fn keypair(&self) -> secp256k1::Keypair {
        self.0
    }
}

/// What an operator has been paid, and can be paid, in one federation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FederationFeeStatus {
    pub federation_id: FederationId,
    /// The account payers must remit to; this is what goes into federation
    /// metadata.
    pub account_id: AccountId,
    /// Deposited and awaiting the next cycle.
    pub staged: Amount,
    /// Committed for the current cycle.
    pub locked: Amount,
    /// Unlocked and withdrawable right now.
    pub idle: Amount,
    /// Total account history entries, of which remittances are a subset.
    pub history_count: u64,
}

impl FederationFeeStatus {
    /// Everything payers have remitted that has not yet been swept out.
    pub fn collectable(&self) -> Amount {
        self.staged + self.locked + self.idle
    }
}

/// Consensus-metadata keys payers read to decide what to charge and where to
/// send it. Both must be set for any fee to be charged.
pub const SEND_PPM_META_KEY: &str = "fedi:guardian_fee_send_ppm";
pub const REMITTANCE_ACCOUNT_META_KEY: &str = "fedi:guardian_fee_remittance_account";

/// Largest payer-compatible rate, in parts per million: 210,000 ppm
/// ([REQ-guardian-fee-remittance](../../../../specs/REQ-guardian-fee-remittance.md)).
/// Reader, proposer, and payer all enforce this bound.
pub use fedi_decentralized_domain::SETUP_PAYMENT_MAX_MIN_FEE_PPM as MAX_SEND_PPM;

/// The FI receives four shares of the ongoing guardian transaction fee.
pub const FI_RECIPIENT_WEIGHT: u64 = FI_GUARDIAN_FEE_WEIGHT;
/// Every accepted guardian receives one share.
pub const GUARDIAN_RECIPIENT_WEIGHT: u64 = GUARDIAN_GUARDIAN_FEE_WEIGHT;
/// The Guardian Verification Fee account receives one share.
pub use fedi_decentralized_service_fleet_manager::GUARDIAN_VERIFICATION_FEE_WEIGHT;

/// Largest recipient list a payer will honour, mirroring the payer's own cap.
/// A longer list is refused there, so treating it as unreadable here keeps the
/// two ends from disagreeing about who gets paid.
pub(crate) const MAX_RECIPIENTS: usize = MAX_GUARDIAN_FEE_RECIPIENTS;

/// What one guarded federation's metadata currently promises this FMan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeePolicy {
    /// Both keys present, so payers will charge and remit. When false, no
    /// revenue is accruing regardless of federation activity.
    pub configured: bool,
    pub send_ppm: Option<u64>,
    /// The raw recipient value, kept so an operator can see exactly what the
    /// federation carries even when this FMan cannot make sense of it.
    pub recipients: Option<String>,
    /// This FMan's weight in the recipient list, and the total across all
    /// recipients. `None` when the value does not name this FMan, does not
    /// parse, or names it in a shape this version does not understand — all of
    /// which mean the same thing operationally: no share is provably ours.
    pub our_share: Option<(u64, u64)>,
}

impl FeePolicy {
    /// Whether the live metadata leaves this guardian's compiled share intact.
    ///
    /// No recipient policy is acceptable: guardian fees are optional. Once a
    /// recipient value exists, however, malformed metadata, omission of this
    /// guardian, and any weight other than the fixed guardian weight are all a
    /// policy violation rather than merely "not currently paying us".
    pub fn share_matches_policy(&self) -> bool {
        self.recipients.is_none()
            || self
                .our_share
                .is_some_and(|(ours, _)| ours == GUARDIAN_RECIPIENT_WEIGHT)
    }
}

/// The payer's versioned recipient list. This mirrors the complete strict
/// entry shape because reporting a locally accepted share from bytes the payer
/// rejects would hide stopped revenue.
#[derive(serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct RecipientList {
    version: u16,
    recipients: Vec<RecipientEntry>,
}

#[derive(serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct RecipientEntry {
    account: Account,
    account_id: String,
    weight: u64,
}

/// Why a guardian-fee metadata value is one the payer will not honour.
#[derive(Debug, thiserror::Error)]
pub enum FeePolicyError {
    #[error("guardian-fee rate and recipient list must be present together")]
    Incomplete,
    #[error("guardian-fee rate exceeds the payer cap of {MAX_SEND_PPM} ppm")]
    SendPpmTooHigh,
    #[error("guardian-fee rate is below the published minimum of {minimum} ppm")]
    SendPpmTooLow { minimum: u64 },
    #[error("guardian-fee recipient list is invalid")]
    InvalidRecipients,
    #[error(
        "guardian-fee recipient list is not the authenticated 4:1:1 split: expected {expected} recipients, got {got}"
    )]
    InvalidSplit { expected: usize, got: usize },
}

/// Validate the guardian-fee values using the payer's acceptance rules.
///
/// Absence of both keys is valid because it disables new guardian fees. A
/// present recipient value accepts either a versioned list or one complete
/// account.
pub fn validate_fee_policy(
    send_ppm: Option<u64>,
    recipients: Option<&str>,
) -> Result<(), FeePolicyError> {
    match (send_ppm, recipients) {
        (None, None) => Ok(()),
        (None, Some(_)) | (Some(_), None) => Err(FeePolicyError::Incomplete),
        (Some(send_ppm), Some(recipients)) => {
            if send_ppm > MAX_SEND_PPM {
                return Err(FeePolicyError::SendPpmTooHigh);
            }
            parse_recipients(recipients)
                .map(|_| ())
                .ok_or(FeePolicyError::InvalidRecipients)
        }
    }
}

/// Derive this FMan's policy view from the consensus meta-map.
///
/// The caller supplies the map from the meta module; this deliberately does
/// not obtain it through a client, so policy reads can use the guardian's own
/// consensus connection rather than a wallet client.
pub fn fee_policy_from_meta(meta: &BTreeMap<String, String>, ours: AccountId) -> FeePolicy {
    let send_ppm = meta
        .get(SEND_PPM_META_KEY)
        .and_then(|value| value.parse::<u64>().ok());
    let recipients = meta.get(REMITTANCE_ACCOUNT_META_KEY).cloned();
    let valid = validate_fee_policy(send_ppm, recipients.as_deref()).is_ok();
    let our_share = valid
        .then(|| {
            recipients
                .as_deref()
                .and_then(|value| our_share_of(value, ours))
        })
        .flatten();
    FeePolicy {
        configured: valid && send_ppm.is_some() && recipients.is_some(),
        send_ppm,
        recipients,
        our_share,
    }
}

/// The recipient value this FMan will vote for, or why it will not.
///
/// This is the whole of the FMan's proposal policy, deliberately in one place:
/// the payer's acceptance rules, the split fixed in this implementation, and
/// the requirement that every account in the consensus-bound seat directory
/// is in the list at the weight that split assigns. A caller that could run
/// only some of these checks would be a way to vote for a list that drops or
/// substitutes a minority guardian
/// ([SPEC-guardian-fee-policy](../../specs/SPEC-guardian-fee-policy.md)).
///
/// `guardians` is the complete FMan-signed account set from the verified
/// consensus seat directory. The complete list additionally contains exactly
/// one residual FI entry at weight four and the Guardian Verification Fee at
/// weight one. FI and FMan identities are disjoint by construction, so there
/// is no combined FI-guardian entry: a weight-five entry, or an FI account
/// colliding with a guardian account, is refused.
pub fn canonical_proposal(
    send_ppm: u64,
    recipients: &[GuardianFeeRecipient],
    guardians: &[Account],
    guardian_verification_fee_account: &Account,
) -> Result<String, FeePolicyError> {
    // `None`: this function is also the carry-forward and revalidation path for
    // a fee policy the federation already adopted, so it must not apply the
    // published minimum. See `prevalidate_guardian_fee_proposal`.
    prevalidate_guardian_fee_proposal(send_ppm, None, recipients)?;
    let expected_normal = guardians.len().saturating_add(2);
    if guardians.is_empty()
        || guardians
            .iter()
            .any(|guardian| guardian == guardian_verification_fee_account)
    {
        return Err(FeePolicyError::InvalidSplit {
            expected: expected_normal,
            got: recipients.len(),
        });
    }

    let parsed = validated_recipient_entries(recipients)?;
    let guardian_verification_fee_entries = parsed
        .iter()
        .filter(|recipient| recipient.account == *guardian_verification_fee_account)
        .collect::<Vec<_>>();
    if !matches!(
        guardian_verification_fee_entries.as_slice(),
        [recipient] if recipient.weight == GUARDIAN_VERIFICATION_FEE_WEIGHT
    ) {
        return Err(FeePolicyError::InvalidSplit {
            expected: expected_normal,
            got: recipients.len(),
        });
    }

    let fi_only = parsed
        .iter()
        .filter(|recipient| {
            recipient.account != *guardian_verification_fee_account
                && !guardians.contains(&recipient.account)
                && recipient.weight == FI_RECIPIENT_WEIGHT
        })
        .count();
    let guardian_weights = guardians
        .iter()
        .map(|guardian| {
            parsed
                .iter()
                .find(|recipient| recipient.account == *guardian)
                .map(|recipient| recipient.weight)
        })
        .collect::<Option<Vec<_>>>();
    let ordinary_guardians = guardian_weights.as_deref().is_some_and(|weights| {
        weights
            .iter()
            .all(|weight| *weight == GUARDIAN_RECIPIENT_WEIGHT)
    });
    if fi_only != 1 || !ordinary_guardians || recipients.len() != expected_normal {
        return Err(FeePolicyError::InvalidSplit {
            expected: expected_normal,
            got: recipients.len(),
        });
    }

    let value = canonical_recipient_list(recipients)?;
    validate_fee_policy(Some(send_ppm), Some(&value))?;
    Ok(value)
}

/// Derive the fixed formation split from authenticated accounts.
pub(crate) fn canonical_formation_proposal(
    send_ppm: u64,
    guardians: &[Account],
    fi: &Account,
    guardian_verification_fee_account: &Account,
) -> Result<String, FeePolicyError> {
    let mut recipients = guardians
        .iter()
        .map(|account| {
            GuardianFeeAccount::try_from(account.clone())
                .map(|account| GuardianFeeRecipient::new(account, GUARDIAN_RECIPIENT_WEIGHT))
                .map_err(|_| FeePolicyError::InvalidRecipients)
        })
        .collect::<Result<Vec<_>, _>>()?;
    recipients.push(GuardianFeeRecipient::new(
        GuardianFeeAccount::try_from(fi.clone()).map_err(|_| FeePolicyError::InvalidRecipients)?,
        FI_RECIPIENT_WEIGHT,
    ));
    recipients.push(GuardianFeeRecipient::new(
        GuardianFeeAccount::try_from(guardian_verification_fee_account.clone())
            .map_err(|_| FeePolicyError::InvalidRecipients)?,
        GUARDIAN_VERIFICATION_FEE_WEIGHT,
    ));
    recipients.sort_by_key(|recipient| recipient.account.as_account().id());
    canonical_proposal(
        send_ppm,
        &recipients,
        guardians,
        guardian_verification_fee_account,
    )
}

/// Revalidate a fee policy already carried by the whole consensus metadata
/// object before this guardian votes for an unrelated field update.
///
/// The meta module adopts one whole object. Without this check, a generic
/// maintenance vote could copy forward fee keys a hostile threshold installed
/// and thereby vote for a payer-valid but policy-invalid recipient split.
pub fn validate_canonical_proposal_value(
    send_ppm: u64,
    value: &str,
    guardians: &[Account],
    guardian_verification_fee_account: &Account,
) -> Result<(), FeePolicyError> {
    let parsed = parse_recipients(value).ok_or(FeePolicyError::InvalidRecipients)?;
    let recipients = parsed
        .into_iter()
        .map(|entry| {
            Ok(GuardianFeeRecipient {
                account: GuardianFeeAccount::try_from(entry.account)
                    .map_err(|_| FeePolicyError::InvalidRecipients)?,
                account_id: entry.account_id,
                weight: entry.weight,
            })
        })
        .collect::<Result<Vec<_>, FeePolicyError>>()?;
    let canonical = canonical_proposal(
        send_ppm,
        &recipients,
        guardians,
        guardian_verification_fee_account,
    )?;
    if canonical != value {
        return Err(FeePolicyError::InvalidRecipients);
    }
    Ok(())
}

/// Bound and type-check the FI-controlled portion of a fee proposal without
/// consulting a child process or federation config.
///
/// The consensus-directory-derived recipient set and complete split are
/// checked later by [`canonical_proposal`], once the live config is available.
///
/// `min_send_ppm` is the published floor, and is `Some` only on the
/// **new-proposal** path. It is deliberately not a property of a fee value in
/// general: [`canonical_proposal`] runs through here again when a guardian
/// merely carries an already-adopted fee policy forward as part of an
/// unrelated meta write, and when the read path reports what a federation
/// currently pays. Enforcing the floor there would make a federation that
/// agreed a lower rate before the floor was raised unmaintainable (a rename or
/// icon change would fail) and would report a paying federation as
/// fee-unconfigured, which
/// [REQ-guardian-fee-remittance](../../../../specs/REQ-guardian-fee-remittance.md)
/// forbids. The floor gates what an FI may newly ask for, nothing else.
pub(crate) fn prevalidate_guardian_fee_rate(
    send_ppm: u64,
    minimum: u64,
) -> Result<(), FeePolicyError> {
    if send_ppm > MAX_SEND_PPM {
        return Err(FeePolicyError::SendPpmTooHigh);
    }
    if send_ppm < minimum {
        return Err(FeePolicyError::SendPpmTooLow { minimum });
    }
    Ok(())
}

pub(crate) fn prevalidate_guardian_fee_proposal(
    send_ppm: u64,
    min_send_ppm: Option<u64>,
    recipients: &[GuardianFeeRecipient],
) -> Result<(), FeePolicyError> {
    if send_ppm > MAX_SEND_PPM {
        return Err(FeePolicyError::SendPpmTooHigh);
    }
    if let Some(minimum) = min_send_ppm
        && send_ppm < minimum
    {
        return Err(FeePolicyError::SendPpmTooLow { minimum });
    }
    canonical_guardian_fee_recipient_list(recipients)
        .map(|_| ())
        .map_err(|_| FeePolicyError::InvalidRecipients)
}

/// Format the FI's recipient entries into the payer's current versioned form.
///
/// The value is canonical before it reaches the meta-map canonicalizer so a
/// guardian never votes for a value that is semantically right but incapable
/// of threshold adoption because another serializer laid it out differently.
fn canonical_recipient_list(recipients: &[GuardianFeeRecipient]) -> Result<String, FeePolicyError> {
    let value = canonical_guardian_fee_recipient_list(recipients)
        .map_err(|_| FeePolicyError::InvalidRecipients)?;
    parse_recipients(&value).ok_or(FeePolicyError::InvalidRecipients)?;
    Ok(value)
}

/// Parse a recipient value exactly as the payer does for the properties this
/// FMan relies on. `None` means the payer will not honour it.
fn parse_recipients(value: &str) -> Option<Vec<RecipientEntry>> {
    if let Ok(list) = serde_json::from_str::<RecipientList>(value) {
        if list.version != 1 {
            return None;
        }
        validate_parsed_entries(&list.recipients)?;
        return Some(list.recipients);
    }

    // A single account represents the whole share without an explicit weight.
    let single = serde_json::from_str::<Account>(value).ok()?;
    Some(vec![RecipientEntry {
        account: single.clone(),
        account_id: single.id().to_string(),
        weight: 1,
    }])
}

fn validated_recipient_entries(
    recipients: &[GuardianFeeRecipient],
) -> Result<Vec<RecipientEntry>, FeePolicyError> {
    let entries = recipients
        .iter()
        .map(|recipient| RecipientEntry {
            account: recipient.account.as_account().clone(),
            account_id: recipient.account_id.clone(),
            weight: recipient.weight,
        })
        .collect::<Vec<_>>();
    validate_parsed_entries(&entries).ok_or(FeePolicyError::InvalidRecipients)?;
    Ok(entries)
}

/// Validate the version-1 weighted recipient wire.
fn validate_parsed_entries(entries: &[RecipientEntry]) -> Option<()> {
    if entries.is_empty() || entries.len() > MAX_RECIPIENTS {
        return None;
    }
    let mut previous = None;
    let mut total = 0_u64;
    for entry in entries {
        if entry.weight == 0
            || entry.account.acc_type() != AccountType::BtcDepositor
            || entry.account.as_single().is_none()
            || entry.account_id != entry.account.id().to_string()
        {
            return None;
        }
        let id = entry.account.id();
        if previous.as_ref().is_some_and(|previous| previous >= &id) {
            return None;
        }
        previous = Some(id);
        total = total.checked_add(entry.weight)?;
    }
    (total != 0).then_some(())
}

/// This FMan's `(weight, total_weight)` in a recipient value, if it names us.
///
/// Two shapes are accepted, matching the payer: the versioned list, and the
/// original single bare account, which is one recipient holding the whole
/// share. An unparsable or over-long value yields `None` rather than an error,
/// because the operator-visible fact is the same either way — nothing here
/// proves a share is ours.
fn our_share_of(value: &str, ours: AccountId) -> Option<(u64, u64)> {
    let recipients = parse_recipients(value)?;
    let total = recipients
        .iter()
        .try_fold(0_u64, |total, entry| total.checked_add(entry.weight))?;
    let ours = recipients.iter().find(|entry| entry.account.id() == ours)?;
    Some((ours.weight, total))
}

/// The complete or incomplete outcome of one collection attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Collected {
    /// Every required operation completed and the resulting balance was read.
    Complete {
        /// Now ordinary ecash in the guardian-fee client.
        claimed: Amount,
        /// Still in the pool and collectable after a cycle turnover.
        awaiting_cycle: Amount,
    },
    /// At least one durable operation exists, but the attempt did not complete.
    Incomplete {
        /// Amount terminally confirmed as ordinary ecash during this attempt.
        confirmed_claimed: Amount,
        /// Current staged plus locked balance, if a post-failure read succeeded.
        observed_awaiting_cycle: Option<Amount>,
        /// The phase that prevented the attempt from completing.
        failure: CollectionFailure,
    },
}

/// Structured details of an incomplete collection attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollectionFailure {
    /// The collection phase that failed.
    pub phase: CollectionFailurePhase,
    /// Whether the failed phase itself returned a durable operation ID.
    pub operation_submitted: bool,
}

/// A phase that can prevent a guardian-fee collection from completing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CollectionFailurePhase {
    /// Claiming balance already idle in the stability pool.
    IdleClaim,
    /// Requesting release of staged and locked stability-pool balance.
    Unlock,
    /// Reading the balance after collection operations.
    BalanceRefresh,
}

/// One remittance a payer made into this FMan's account.
#[derive(Debug, Clone)]
pub struct Remittance {
    pub amount: Amount,
    pub txid: String,
    /// The payer's sealed accounting breakdown. An `Err` here is a readable
    /// deposit with unreadable paperwork — the money is still ours, so the
    /// entry is reported rather than dropped.
    pub metadata: Result<RemittanceMetadata, String>,
}

/// How many history entries to ask for in one read while walking an account's
/// history. A page size is a network cost, never a semantic one: no answer
/// below depends on it.
const HISTORY_PAGE: u64 = 128;

/// One seat's remittance-account history, as the vault is able to read it.
///
/// The vault holds the Fedimint client, so it owns the reads. How far back to
/// read, and what the entries add up to, is arithmetic — and it lives here,
/// where a test can drive it over a fabricated history without a federation.
/// That is the whole point of the seam: truncation is a property of the walk,
/// not of the client, so the walk has to be reachable from a test.
#[async_trait::async_trait]
pub trait AccountHistory: Send + Sync {
    /// How many entries the account has right now — the same number
    /// [`FederationFeeStatus::history_count`] reports.
    async fn count(&self) -> anyhow::Result<u64>;

    /// The entries at indices `range`, oldest first.
    async fn page(&self, range: Range<u64>) -> anyhow::Result<Vec<AccountHistoryItem>>;
}

/// Everything payers have ever remitted into this account, including what has
/// already been collected out of the pool and swept away.
///
/// **Computed over the full history, not maintained as a counter, and that is
/// a deliberate choice rather than an omission.** The balances beside it
/// (`staged`/`locked`/`idle`) are maintained — by the stability-pool server,
/// which hands them over in one sync read. That server keeps no lifetime
/// deposit aggregate, and nothing inside this daemon ever observes a
/// remittance as it lands: payers deposit into the pool, not into anything
/// FMan runs, so there is no event a counter could be incremented from. A
/// daemon-side counter would therefore be a cache of exactly this walk, and
/// would have to be rebuilt by performing it on any restore from mnemonic —
/// which is the one recovery path guardian fees are required to survive. The
/// walk is the ground truth, so the walk is what is reported.
///
/// It is also deliberately not a sum of [`recent_remittances`]. That list is
/// windowed for display; totalling a window is the fault this exists to
/// remove.
pub async fn total_remitted(history: &dyn AccountHistory) -> anyhow::Result<Amount> {
    let mut total = Amount::ZERO;
    walk_deposits(history, u64::MAX, |item, _sealed| {
        total = total
            .checked_add(item.amount)
            .ok_or_else(|| anyhow::anyhow!("guardian-fee lifetime total exceeds u64 msats"))?;
        Ok(())
    })
    .await?;
    Ok(total)
}

/// The newest `limit` remittances into this account, newest first, with each
/// payer's sealed breakdown opened.
///
/// A display window by contract: the caller states how many entries it wants
/// to show. Nothing here is an aggregate, and nothing that adds these up is a
/// lifetime figure — see [`total_remitted`] for that.
pub async fn recent_remittances(
    history: &dyn AccountHistory,
    key: &GuardianFeeAccountKey,
    limit: u64,
) -> anyhow::Result<Vec<Remittance>> {
    let account_key = key.keypair();
    let mut remittances = Vec::new();
    walk_deposits(history, limit, |item, sealed| {
        remittances.push(Remittance {
            amount: item.amount,
            txid: item.txid.to_string(),
            metadata: crate::remittance_metadata::decrypt(&account_key, sealed)
                .map_err(|err| format!("{err:#}")),
        });
        Ok(())
    })
    .await?;
    Ok(remittances)
}

/// Walk the account's history newest-first, handing every btc-balance deposit
/// to `take` until it has had `limit` of them or the history runs out.
///
/// Reading backwards a page at a time is what the limit is for: remittances
/// are a subset of history, so the newest few can sit behind any number of
/// unrelated entries, and a caller that wants them all must be able to say so
/// without guessing a number. `u64::MAX` means the whole history, and reaching
/// index zero is the only thing that ends that walk.
async fn walk_deposits(
    history: &dyn AccountHistory,
    limit: u64,
    mut take: impl FnMut(&AccountHistoryItem, &[u8]) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    let mut taken = 0_u64;
    let mut end = history.count().await?;
    while end > 0 && taken < limit {
        let start = end.saturating_sub(HISTORY_PAGE);
        for item in history.page(start..end).await?.into_iter().rev() {
            let AccountHistoryItemKind::DepositToBtcBalance { ref metadata } = item.kind else {
                continue;
            };
            take(&item, &metadata.0)?;
            taken += 1;
            if taken >= limit {
                break;
            }
        }
        end = start;
    }
    Ok(())
}

/// Moving guardian-fee revenue: the half of fees that is wiring over
/// existing primitives, filled by whatever wallet can hold a client in the
/// guarded federation ([`crate::wallet::EcashWallet::guardian_fees`]).
///
/// Every method is handed the seat's [`GuardianFeeAccountKey`] rather than
/// deriving or storing one: the account is core's answer, and an
/// implementation that could choose its own would be able to sweep fees
/// into somewhere the FI was never told about.
///
/// Collection can require multiple durable operations. Once one operation
/// exists, [`GuardianFeeVault::collect`] reports later failure as
/// [`Collected::Incomplete`] so callers retain exact confirmed progress.
#[async_trait::async_trait]
pub trait GuardianFeeVault: Send + Sync {
    /// Current balances and account identity for one guarded federation.
    async fn status(
        &self,
        invite_code: &InviteCode,
        seat_id: &SeatId,
        key: &GuardianFeeAccountKey,
    ) -> anyhow::Result<FederationFeeStatus>;

    /// The most recent remittances into this seat's account, newest first.
    async fn remittances(
        &self,
        invite_code: &InviteCode,
        seat_id: &SeatId,
        key: &GuardianFeeAccountKey,
        limit: u64,
    ) -> anyhow::Result<Vec<Remittance>>;

    /// Everything payers have ever remitted into this seat's account,
    /// including what has already been collected out of the pool and swept
    /// away.
    ///
    /// Deliberately separate from [`Self::remittances`]: that list is
    /// windowed for display, and a total taken from a window is a lifetime
    /// figure that silently stops counting.
    async fn total_remitted(
        &self,
        invite_code: &InviteCode,
        seat_id: &SeatId,
        key: &GuardianFeeAccountKey,
    ) -> anyhow::Result<Amount>;

    /// Move what the pool will release now into ordinary ecash.
    async fn collect(
        &self,
        invite_code: &InviteCode,
        seat_id: &SeatId,
        key: &GuardianFeeAccountKey,
    ) -> anyhow::Result<Collected>;

    /// Ecash already collected out of the pool and awaiting a sweep.
    async fn ecash_balance(
        &self,
        invite_code: &InviteCode,
        seat_id: &SeatId,
        key: &GuardianFeeAccountKey,
    ) -> anyhow::Result<Amount>;
}

#[cfg(test)]
#[path = "../tests/fee_policy.rs"]
mod fee_policy_tests;

#[cfg(test)]
#[path = "../tests/guardian_fee_history.rs"]
mod guardian_fee_history_tests;
