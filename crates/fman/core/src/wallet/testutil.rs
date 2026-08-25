//! Test wallets shared by fleet test suites (behind the `testutil` feature).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use fedi_decentralized_service_fleet_manager::{
    FederationId, LockedBlindedSignature, PaymentTerms, QuoteId, QuoteTerms, RefundIssuance,
    RefundTransaction,
};
use tokio::sync::Semaphore;

use super::{
    ClaimOutcome, EcashWallet, LockedPaymentPrepareError, Msats, NoWallet, VerifiedLockedPayment,
};

/// Counts payment hand-offs and durable fake payout starts. Refund submits are
/// held until the test grants a permit, so replays can land while a refund is
/// still in flight. Payout lookup survives rebuilding `Fleet` around this
/// wallet, and payout observation returns a deterministic terminal projection.
pub struct GatedRefundWallet {
    submits: AtomicUsize,
    gate: Semaphore,
}

impl Default for GatedRefundWallet {
    fn default() -> Self {
        Self::new()
    }
}

impl GatedRefundWallet {
    pub fn new() -> Self {
        Self {
            submits: AtomicUsize::new(0),
            gate: Semaphore::new(0),
        }
    }

    pub fn settling(_outcome: ClaimOutcome) -> Self {
        Self {
            submits: AtomicUsize::new(0),
            gate: Semaphore::new(0),
        }
    }

    /// How many refund submits have started (including still-gated ones).
    pub fn submit_count(&self) -> usize {
        self.submits.load(Ordering::SeqCst)
    }

    /// Let one gated submit finish.
    pub fn release_one(&self) {
        self.gate.add_permits(1);
    }

    /// Wait (bounded) until `expected` refund submits have started.
    pub async fn wait_for_submits(&self, expected: usize) {
        tokio::time::timeout(Duration::from_secs(5), async {
            while self.submit_count() < expected {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("never reached {expected} refund submits"));
    }
}

#[async_trait::async_trait]
impl EcashWallet for GatedRefundWallet {
    async fn quote_locked(
        &self,
        federation_id: &FederationId,
        price: Msats,
        quote_nonce: &[u8; 32],
    ) -> Result<PaymentTerms, LockedPaymentPrepareError> {
        NoWallet
            .quote_locked(federation_id, price, quote_nonce)
            .await
    }

    async fn validate_quote_refund(
        &self,
        payment: &PaymentTerms,
        refund: &RefundIssuance,
    ) -> Result<(), LockedPaymentPrepareError> {
        NoWallet.validate_quote_refund(payment, refund).await
    }

    async fn verify_locked(
        &self,
        quote_id: &QuoteId,
        terms: &QuoteTerms,
        payment_signatures: &[LockedBlindedSignature],
    ) -> Result<VerifiedLockedPayment, LockedPaymentPrepareError> {
        NoWallet
            .verify_locked(quote_id, terms, payment_signatures)
            .await
    }

    async fn submit_refund_transaction(
        &self,
        _federation_id: &FederationId,
        _transaction: &RefundTransaction,
    ) -> anyhow::Result<()> {
        self.submits.fetch_add(1, Ordering::SeqCst);
        self.gate
            .acquire()
            .await
            .expect("test gate is never closed")
            .forget();
        Ok(())
    }

    async fn receivable(&self, federation_id: &FederationId) -> bool {
        NoWallet.receivable(federation_id).await
    }

    async fn joined_federation_ids(&self) -> Vec<FederationId> {
        NoWallet.joined_federation_ids().await
    }

    async fn join(&self, invite_code: &str) -> anyhow::Result<FederationId> {
        NoWallet.join(invite_code).await
    }

    fn guardian_fees(&self) -> Option<&dyn crate::guardian_fee::GuardianFeeVault> {
        Some(self)
    }
}

#[async_trait::async_trait]
impl crate::guardian_fee::GuardianFeeVault for GatedRefundWallet {
    async fn status(
        &self,
        _invite_code: &fedimint_core::invite_code::InviteCode,
        _seat_id: &fedi_decentralized_service_fleet_manager::SeatId,
        _key: &crate::guardian_fee::GuardianFeeAccountKey,
    ) -> anyhow::Result<crate::guardian_fee::FederationFeeStatus> {
        anyhow::bail!("unused in payout tests")
    }

    async fn remittances(
        &self,
        _invite_code: &fedimint_core::invite_code::InviteCode,
        _seat_id: &fedi_decentralized_service_fleet_manager::SeatId,
        _key: &crate::guardian_fee::GuardianFeeAccountKey,
        _limit: u64,
    ) -> anyhow::Result<Vec<crate::guardian_fee::Remittance>> {
        anyhow::bail!("unused in payout tests")
    }

    async fn total_remitted(
        &self,
        _invite_code: &fedimint_core::invite_code::InviteCode,
        _seat_id: &fedi_decentralized_service_fleet_manager::SeatId,
        _key: &crate::guardian_fee::GuardianFeeAccountKey,
    ) -> anyhow::Result<fedimint_core::Amount> {
        anyhow::bail!("unused in payout tests")
    }

    async fn collect(
        &self,
        _invite_code: &fedimint_core::invite_code::InviteCode,
        _seat_id: &fedi_decentralized_service_fleet_manager::SeatId,
        _key: &crate::guardian_fee::GuardianFeeAccountKey,
    ) -> anyhow::Result<crate::guardian_fee::Collected> {
        anyhow::bail!("unused in payout tests")
    }

    async fn ecash_balance(
        &self,
        _invite_code: &fedimint_core::invite_code::InviteCode,
        _seat_id: &fedi_decentralized_service_fleet_manager::SeatId,
        _key: &crate::guardian_fee::GuardianFeeAccountKey,
    ) -> anyhow::Result<fedimint_core::Amount> {
        anyhow::bail!("unused in payout tests")
    }
}
