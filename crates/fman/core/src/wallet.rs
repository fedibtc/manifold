//! The fleet-facing payment boundary: protocol types on one side, an ecash
//! wallet on the other. Fleet logic sees ecash receive semantics only
//! through [`EcashWallet`]; which mint generation a quote uses, and every
//! piece of cryptography behind it, belong to the implementation — the one
//! that ships is `fman-fedimint`, which this crate deliberately
//! does not depend on.
//!
//! The vocabulary lives here, above the implementation, because it is what
//! the fleet persists (typed claim evidence and outcomes) and prices in
//! ([`Msats`]) — storage and policy must not import their own shapes from
//! the capability that happens to fill the hole.

#[cfg(test)]
pub mod testutil;

use fedi_decentralized_service_fleet_manager::{
    FederationId, InviteCode, LockedBlindedSignature, LockedIssuanceRequest,
    LockedIssuanceRequestV2, PaymentTerms, QuoteId, QuoteTerms, RefundIssuance, RefundTransaction,
};
use fedimint_core::core::ModuleInstanceId;

use crate::guardian_fee::GuardianFeeVault;
use std::fmt;
use std::str::FromStr;

use crate::guardian_fee::GuardianFeeAccountKey;
use crate::payout_wire::{PayoutJobStatusWire, PayoutJobWire, WalletDrainStatusWire};
use fedi_decentralized_service_fleet_manager::SeatId;

/// Handle to the implementation-owned reconciler for accepted ecash claims.
/// SQLite is authoritative; [`Self::mark`] is only a prompt rescan hint.
#[async_trait::async_trait]
pub trait EcashClaimWorker: Send + Sync + 'static {
    fn mark(&self);
    async fn shutdown(&self);
}

/// An opaque caller-generated identity for one payout request.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, serde::Serialize)]
#[serde(transparent)]
pub struct PayoutRequestId(String);

impl PayoutRequestId {
    pub fn parse(value: &str) -> anyhow::Result<Self> {
        anyhow::ensure!(!value.is_empty(), "payout request id is empty");
        anyhow::ensure!(
            value.len() <= 128,
            "payout request id is longer than 128 bytes"
        );
        anyhow::ensure!(
            !value.chars().any(char::is_control),
            "payout request id contains a control character"
        );
        Ok(Self(value.to_owned()))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl fmt::Display for PayoutRequestId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}
impl FromStr for PayoutRequestId {
    type Err = anyhow::Error;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}
impl<'de> serde::Deserialize<'de> for PayoutRequestId {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let value = String::deserialize(d)?;
        Self::parse(&value).map_err(serde::de::Error::custom)
    }
}

/// Intent-level operator payout boundary. The implementation owns its durable
/// ledger and native-wallet projection and returns stable operator DTOs.
#[async_trait::async_trait]
pub trait EcashPayoutWorker: Send + Sync + 'static {
    async fn sweep_payment_fees(
        &self,
        federation_id: &FederationId,
        request_id: &PayoutRequestId,
    ) -> anyhow::Result<PayoutJobWire>;
    async fn sweep_guardian_fees(
        &self,
        invite_code: &InviteCode,
        seat_id: &SeatId,
        request_id: &PayoutRequestId,
    ) -> anyhow::Result<PayoutJobWire>;
    /// Resume a durably scoped guardian sweep without consulting the live seat.
    /// `None` means this request has no job yet and needs a fresh invite.
    async fn resume_guardian_sweep(
        &self,
        seat_id: &SeatId,
        request_id: &PayoutRequestId,
    ) -> anyhow::Result<Option<PayoutJobWire>>;
    async fn payout_status(
        &self,
        request_id: &PayoutRequestId,
    ) -> anyhow::Result<PayoutJobStatusWire>;
    async fn await_payout(
        &self,
        request_id: &PayoutRequestId,
    ) -> anyhow::Result<PayoutJobStatusWire>;
    async fn payment_drain_status(&self, federation_id: &FederationId) -> WalletDrainStatusWire;
    async fn guardian_drain_status(
        &self,
        invite_code: &InviteCode,
        seat_id: &SeatId,
    ) -> anyhow::Result<WalletDrainStatusWire>;
}

struct NoEcashPayoutWorker;
#[async_trait::async_trait]
impl EcashPayoutWorker for NoEcashPayoutWorker {
    async fn sweep_payment_fees(
        &self,
        _: &FederationId,
        _: &PayoutRequestId,
    ) -> anyhow::Result<PayoutJobWire> {
        anyhow::bail!("no wallet is configured")
    }
    async fn sweep_guardian_fees(
        &self,
        _: &InviteCode,
        _: &SeatId,
        _: &PayoutRequestId,
    ) -> anyhow::Result<PayoutJobWire> {
        anyhow::bail!("no wallet is configured")
    }
    async fn resume_guardian_sweep(
        &self,
        _: &SeatId,
        _: &PayoutRequestId,
    ) -> anyhow::Result<Option<PayoutJobWire>> {
        Ok(None)
    }
    async fn payout_status(&self, _: &PayoutRequestId) -> anyhow::Result<PayoutJobStatusWire> {
        anyhow::bail!("no wallet is configured")
    }
    async fn await_payout(&self, _: &PayoutRequestId) -> anyhow::Result<PayoutJobStatusWire> {
        anyhow::bail!("no wallet is configured")
    }
    async fn payment_drain_status(&self, _: &FederationId) -> WalletDrainStatusWire {
        WalletDrainStatusWire::unavailable()
    }
    async fn guardian_drain_status(
        &self,
        _: &InviteCode,
        _: &SeatId,
    ) -> anyhow::Result<WalletDrainStatusWire> {
        Ok(WalletDrainStatusWire::unavailable())
    }
}

struct NoEcashClaimWorker;

#[async_trait::async_trait]
impl EcashClaimWorker for NoEcashClaimWorker {
    fn mark(&self) {}
    async fn shutdown(&self) {}
}

/// The 64-byte wallet root secret. Everything an ecash wallet owns —
/// federation clients, locked-quote note keys — derives from it; hold it
/// only long enough to hand to the wallet implementation. Deliberately not
/// `Debug`/`Display`.
pub struct WalletSecret(pub [u8; 64]);

/// An ecash amount, in millisatoshis. Daemon-internal: the wire carries
/// bare `u64` millisatoshis, and this is the type that gives them a name
/// inside the wallet boundary and the underpayment check.
#[derive(
    serde::Deserialize, serde::Serialize, Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd,
)]
pub struct Msats(pub u64);

/// Terminal result reported by fedimint's receive state machine.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClaimOutcome {
    Success,
    /// The federation terminally refused reissuing the verified locked notes,
    /// so the claim records their inputs as already spent. Claim evidence exists
    /// only after offline verification at `CreateSeat`, and the notes are
    /// spendable only by keys derived from this FMan's mnemonic. A prior
    /// successful claim by this identity (notably before a mnemonic restore) is
    /// therefore the expected cause, but this name states the observed input
    /// state rather than inferring who spent them. The upstream terminal state
    /// can also represent another transaction or output-finalization failure,
    /// which this coarse outcome cannot distinguish.
    AlreadySpent,
}

#[derive(Debug, thiserror::Error)]
pub enum LockedPaymentPrepareError {
    #[error("invalid key-locked payment")]
    Invalid,
    #[error("payment federation temporarily unavailable")]
    Unavailable,
    #[error(transparent)]
    Internal(#[from] anyhow::Error),
}

/// An offline-verified locked payment, as the fleet holds it: typed
/// accepted-claim evidence and a lazy claw-back builder, and nothing else.
///
/// The fleet decides *whether* to refund, never how. It cannot fail to
/// produce the bytes once it has decided. The transaction is built only after
/// refusal; acceptance consumes this value into claim evidence instead, so an
/// accepted payment does no unnecessary signing work.
///
/// The mint generation is explicit in the claim evidence below. This value is
/// consuming, so one verified payment can become either an accepted claim or a
/// refund transaction, never both.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(tag = "mint", rename_all = "snake_case", deny_unknown_fields)]
pub enum EcashClaimEvidence {
    MintV1 {
        /// Names the payment federation and how to rejoin it; the federation
        /// id a claim needs is embedded in the invite rather than duplicated.
        federation_invite: InviteCode,
        module_id: ModuleInstanceId,
        quote_nonce: [u8; 32],
        issuance: Vec<LockedIssuanceRequest>,
        signatures: Vec<LockedBlindedSignature>,
    },
    MintV2 {
        /// Names the payment federation and how to rejoin it; the federation
        /// id a claim needs is embedded in the invite rather than duplicated.
        federation_invite: InviteCode,
        module_id: ModuleInstanceId,
        issuance: Vec<LockedIssuanceRequestV2>,
        signatures: Vec<LockedBlindedSignature>,
    },
}

#[cfg(test)]
impl EcashClaimEvidence {
    pub(crate) fn test(marker: u8) -> Self {
        Self::MintV1 {
            federation_invite: InviteCode(format!("test-invite-{marker}")),
            module_id: 0,
            quote_nonce: [marker; 32],
            issuance: Vec::new(),
            signatures: Vec::new(),
        }
    }
}

pub struct VerifiedLockedPayment {
    refund: Box<dyn FnOnce() -> RefundTransaction + Send>,
    claim: EcashClaimEvidence,
}

impl VerifiedLockedPayment {
    /// Called only by an [`EcashWallet`] that has just verified the payment
    /// these bytes claw back.
    pub fn new(
        claim: EcashClaimEvidence,
        refund: impl FnOnce() -> RefundTransaction + Send + 'static,
    ) -> Self {
        Self {
            refund: Box::new(refund),
            claim,
        }
    }

    pub fn into_claim_evidence(self) -> EcashClaimEvidence {
        self.claim
    }

    pub(crate) fn claim_evidence(&self) -> &EcashClaimEvidence {
        &self.claim
    }

    pub fn into_refund_transaction(self) -> RefundTransaction {
        (self.refund)()
    }
}

/// What the fleet needs from an ecash wallet. The mint generation is the
/// wallet's own affair: the fleet only routes [`PaymentTerms`] and the
/// presented signatures between the wire and this boundary.
#[async_trait::async_trait]
pub trait EcashWallet: Send + Sync + 'static {
    /// Start this implementation's claim reconciler against the fleet's shared
    /// SQLite database. Implementations with no claim machinery stay inert.
    fn start_claim_worker(
        self: std::sync::Arc<Self>,
        _db: crate::db::Db,
    ) -> std::sync::Arc<dyn EcashClaimWorker> {
        std::sync::Arc::new(NoEcashClaimWorker)
    }

    fn start_payout_worker(
        self: std::sync::Arc<Self>,
        _db: crate::db::Db,
        _guardian_key: std::sync::Arc<dyn Fn(&SeatId) -> GuardianFeeAccountKey + Send + Sync>,
    ) -> std::sync::Arc<dyn EcashPayoutWorker> {
        std::sync::Arc::new(NoEcashPayoutWorker)
    }

    /// Quote a locked payment covering `price` on one accepted federation.
    /// `quote_nonce` is the quote's public randomness; per-note secrets
    /// derive statelessly from it and the wallet root, so the returned
    /// terms are the complete quote — nothing needs escrowing.
    async fn quote_locked(
        &self,
        federation_id: &FederationId,
        price: Msats,
        quote_nonce: &[u8; 32],
    ) -> Result<PaymentTerms, LockedPaymentPrepareError>;

    async fn validate_quote_refund(
        &self,
        payment: &PaymentTerms,
        refund: &RefundIssuance,
    ) -> Result<(), LockedPaymentPrepareError>;

    /// Verify a presented payment offline against its quoted terms. The
    /// presented kind must match the quoted generation; the per-note escrow
    /// secrets are re-derived from the wallet root and the quote nonce,
    /// never transported, so terms this wallet did not derive fail
    /// verification. The refund transaction's nonce derives from `quote_id`
    /// (the hash of the signed quote bytes), keeping the refusal path's
    /// refund bytes deterministic per quote.
    async fn verify_locked(
        &self,
        quote_id: &QuoteId,
        terms: &QuoteTerms,
        payment_signatures: &[LockedBlindedSignature],
    ) -> Result<VerifiedLockedPayment, LockedPaymentPrepareError>;

    async fn submit_refund_transaction(
        &self,
        federation_id: &FederationId,
        transaction: &RefundTransaction,
    ) -> anyhow::Result<()>;

    /// Whether an OOB receive in this federation is currently plausible.
    /// Gates paid-plan availability (advertised slots), never free plans.
    async fn receivable(&self, federation_id: &FederationId) -> bool;

    /// Every payment-federation client open in this process. Removed members
    /// stay sweepable only while their retained clients remain open; use
    /// [`Self::retained_federation_ids`] to discover dormant scopes after restart.
    async fn joined_federation_ids(&self) -> Vec<FederationId>;

    /// Every payment-federation scope durably retained by the wallet, including
    /// scopes which have not been reopened in this process.
    async fn retained_federation_ids(&self) -> Vec<FederationId> {
        self.joined_federation_ids().await
    }

    // Operator-side operations (the admin socket verbs). These answer the
    // operator directly, so failures are human-readable strings, not
    // protocol errors.

    /// Join a payment federation by invite code; idempotent. Returns the
    /// joined federation's id.
    async fn join(&self, invite_code: &str) -> anyhow::Result<FederationId>;

    /// Guardian-fee collection, when this wallet can also act as the
    /// recipient side of federation usage fees — it needs an ordinary
    /// client in each guarded federation, which is what a payment wallet
    /// already knows how to build. `None` (the default) means no fee
    /// revenue can be read or moved, and every fee verb says so; a wallet
    /// that never returns `Some` cannot be asked in any other way.
    fn guardian_fees(&self) -> Option<&dyn GuardianFeeVault> {
        None
    }
}

/// A fleet with no wallet wired (tests of free flows, or a free-only FMan):
/// nothing is receivable and every receive fails.
pub struct NoWallet;

#[async_trait::async_trait]
impl EcashWallet for NoWallet {
    async fn quote_locked(
        &self,
        _federation_id: &FederationId,
        _price: Msats,
        _quote_nonce: &[u8; 32],
    ) -> Result<PaymentTerms, LockedPaymentPrepareError> {
        Err(LockedPaymentPrepareError::Internal(anyhow::anyhow!(
            "no wallet is configured"
        )))
    }

    async fn validate_quote_refund(
        &self,
        _payment: &PaymentTerms,
        _refund: &RefundIssuance,
    ) -> Result<(), LockedPaymentPrepareError> {
        Err(LockedPaymentPrepareError::Internal(anyhow::anyhow!(
            "no wallet is configured"
        )))
    }

    async fn verify_locked(
        &self,
        _quote_id: &QuoteId,
        _terms: &QuoteTerms,
        _payment_signatures: &[LockedBlindedSignature],
    ) -> Result<VerifiedLockedPayment, LockedPaymentPrepareError> {
        Err(LockedPaymentPrepareError::Internal(anyhow::anyhow!(
            "no wallet is configured"
        )))
    }

    async fn submit_refund_transaction(
        &self,
        _federation_id: &FederationId,
        _transaction: &RefundTransaction,
    ) -> anyhow::Result<()> {
        anyhow::bail!("no wallet is configured")
    }

    async fn receivable(&self, _federation_id: &FederationId) -> bool {
        false
    }

    async fn joined_federation_ids(&self) -> Vec<FederationId> {
        Vec::new()
    }

    async fn retained_federation_ids(&self) -> Vec<FederationId> {
        Vec::new()
    }

    async fn join(&self, _invite_code: &str) -> anyhow::Result<FederationId> {
        anyhow::bail!("no wallet is configured")
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    #[test]
    fn refund_signing_is_lazy_and_only_the_refusal_path_invokes_it() {
        let calls = Arc::new(AtomicUsize::new(0));
        let payment = VerifiedLockedPayment::new(EcashClaimEvidence::test(1), {
            let calls = calls.clone();
            move || {
                calls.fetch_add(1, Ordering::SeqCst);
                RefundTransaction(vec![1])
            }
        });
        assert_eq!(payment.into_claim_evidence(), EcashClaimEvidence::test(1));
        assert_eq!(calls.load(Ordering::SeqCst), 0);

        let payment = VerifiedLockedPayment::new(EcashClaimEvidence::test(2), {
            let calls = calls.clone();
            move || {
                calls.fetch_add(1, Ordering::SeqCst);
                RefundTransaction(vec![2])
            }
        });
        assert_eq!(
            payment.into_refund_transaction(),
            RefundTransaction(vec![2])
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}
