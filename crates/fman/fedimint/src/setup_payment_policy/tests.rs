use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use fedi_decentralized_service_fleet_manager::{
    FederationId, LockedBlindedSignature, PaymentTerms, QuoteId, QuoteTerms, RefundIssuance,
    RefundTransaction,
};
use fedimint_core::PeerId;
use fedimint_core::invite_code::InviteCode as FedimintInviteCode;
use fedimint_core::util::SafeUrl;
use tokio::sync::watch;

use super::*;
use fman_core::wallet::{LockedPaymentPrepareError, Msats, NoWallet, VerifiedLockedPayment};

fn invite_for_id(index: usize) -> (String, String) {
    let federation_id: fedimint_core::config::FederationId = format!("{index:064x}")
        .parse()
        .expect("test federation ID is valid");
    let invite = FedimintInviteCode::new(
        SafeUrl::parse(&format!("https://{index}.example/")).expect("test URL is valid"),
        PeerId::from(0),
        federation_id,
        None,
    );
    (federation_id.to_string(), invite.to_string())
}

fn admitted_set(indices: &[usize]) -> AdmittedSetupPaymentFederations {
    let invites: Vec<String> = indices
        .iter()
        .map(|index| invite_for_id(*index).1)
        .collect();
    let content = serde_json::json!({
        "version": 1,
        "fman_version": "0.1.0",
        "federations": invites,
        "telemetry_registration_url": "https://push.fedi.example/v1/telemetry/registrations",
    });
    AdmittedSetupPaymentFederations::parse(content.to_string().as_bytes())
        .expect("test set is admissible")
}

/// Join-recording wallet: joins succeed unless all joins are marked failing or
/// the invite's federation is configured to remain pending. Successful joins
/// become durable membership.
#[derive(Default)]
struct RecordingWallet {
    joined: std::sync::Mutex<BTreeSet<String>>,
    join_calls: std::sync::Mutex<Vec<String>>,
    fail_joins: AtomicBool,
    pending_joins: std::sync::Mutex<BTreeSet<String>>,
    cancelled_joins: std::sync::Mutex<BTreeSet<String>>,
    active_joins: AtomicUsize,
    max_active_joins: AtomicUsize,
}

struct ActiveJoin<'a>(&'a AtomicUsize);

impl Drop for ActiveJoin<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

struct PendingJoin<'a> {
    federation_id: String,
    cancelled_joins: &'a std::sync::Mutex<BTreeSet<String>>,
}

impl Drop for PendingJoin<'_> {
    fn drop(&mut self) {
        self.cancelled_joins
            .lock()
            .unwrap()
            .insert(self.federation_id.clone());
    }
}

#[async_trait::async_trait]
impl EcashWallet for RecordingWallet {
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
        federation_id: &FederationId,
        transaction: &RefundTransaction,
    ) -> anyhow::Result<()> {
        NoWallet
            .submit_refund_transaction(federation_id, transaction)
            .await
    }

    async fn receivable(&self, federation_id: &FederationId) -> bool {
        self.joined.lock().unwrap().contains(&federation_id.0)
    }

    async fn joined_federation_ids(&self) -> Vec<FederationId> {
        self.joined
            .lock()
            .unwrap()
            .iter()
            .cloned()
            .map(FederationId)
            .collect()
    }

    async fn join(&self, invite_code: &str) -> anyhow::Result<FederationId> {
        let invite: FedimintInviteCode = invite_code.parse()?;
        let federation_id = invite.federation_id().to_string();
        self.join_calls.lock().unwrap().push(federation_id.clone());
        let active = self.active_joins.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_active_joins.fetch_max(active, Ordering::SeqCst);
        let _active = ActiveJoin(&self.active_joins);
        let remains_pending = self.pending_joins.lock().unwrap().contains(&federation_id);
        if remains_pending {
            let _pending = PendingJoin {
                federation_id: federation_id.clone(),
                cancelled_joins: &self.cancelled_joins,
            };
            std::future::pending().await
        }
        if self.fail_joins.load(Ordering::SeqCst) {
            anyhow::bail!("injected join failure");
        }
        self.joined.lock().unwrap().insert(federation_id.clone());
        Ok(FederationId(federation_id))
    }
}

async fn wait_until(budget: std::time::Duration, mut condition: impl AsyncFnMut() -> bool) {
    tokio::time::timeout(budget, async {
        while !condition().await {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("condition within test budget");
}

fn fresh_attempted_federation_ids() -> AttemptedFederationIds {
    Arc::new(std::sync::Mutex::new(BTreeSet::new()))
}

#[tokio::test]
async fn joins_every_member_and_only_missing_members_on_update() {
    let wallet = Arc::new(RecordingWallet::default());
    let (sender, receiver) = watch::channel(Some(admitted_set(&[1, 2])));
    let task = spawn_setup_payment_join_reconciler_with_attempts(
        wallet.clone(),
        receiver,
        fresh_attempted_federation_ids(),
    );

    wait_until(std::time::Duration::from_secs(5), async || {
        wallet.joined.lock().unwrap().len() == 2
    })
    .await;

    // A grown set joins only the new member; existing members are not
    // re-joined.
    let calls_before = wallet.join_calls.lock().unwrap().len();
    sender.send(Some(admitted_set(&[1, 2, 3]))).unwrap();
    wait_until(std::time::Duration::from_secs(5), async || {
        wallet.joined.lock().unwrap().len() == 3
    })
    .await;
    assert_eq!(wallet.join_calls.lock().unwrap().len(), calls_before + 1);

    // A shrunk set never leaves: wallet state for the removed member stays.
    sender.send(Some(admitted_set(&[1]))).unwrap();
    sender.send(Some(admitted_set(&[1, 3]))).unwrap();
    wait_until(std::time::Duration::from_secs(5), async || {
        *sender.borrow() == Some(admitted_set(&[1, 3]))
    })
    .await;
    assert_eq!(wallet.joined.lock().unwrap().len(), 3);

    drop(sender);
    tokio::time::timeout(std::time::Duration::from_secs(5), task.join())
        .await
        .expect("reconciler exits when policy sender drops")
        .expect("reconciler task exits cleanly");
}

#[tokio::test(start_paused = true)]
async fn failed_joins_wait_for_restart_but_new_members_are_attempted() {
    let wallet = Arc::new(RecordingWallet::default());
    let failed_id = invite_for_id(7).0;
    let new_id = invite_for_id(8).0;
    let readmitted_companion_id = invite_for_id(9).0;
    let replacement_companion_id = invite_for_id(10).0;
    let attempted_federation_ids = fresh_attempted_federation_ids();
    wallet.fail_joins.store(true, Ordering::SeqCst);
    let (sender, receiver) = watch::channel(Some(admitted_set(&[7])));
    let task = spawn_setup_payment_join_reconciler_with_attempts(
        wallet.clone(),
        receiver,
        attempted_federation_ids.clone(),
    );

    wait_until(std::time::Duration::from_secs(5), async || {
        !wallet.join_calls.lock().unwrap().is_empty()
    })
    .await;
    assert!(wallet.joined.lock().unwrap().is_empty());

    wallet.fail_joins.store(false, Ordering::SeqCst);
    tokio::time::advance(std::time::Duration::from_secs(61)).await;
    tokio::task::yield_now().await;
    assert_eq!(
        wallet
            .join_calls
            .lock()
            .unwrap()
            .iter()
            .filter(|id| id.as_str() == failed_id)
            .count(),
        1,
        "time passing must not retry a failed join"
    );

    // Process a policy that removes the failed ID, then re-admit it alongside
    // another new ID. Neither transition resets its process-lifetime attempt.
    sender.send(Some(admitted_set(&[8]))).unwrap();
    wait_until(std::time::Duration::from_secs(5), async || {
        wallet.joined.lock().unwrap().contains(&new_id)
    })
    .await;
    sender.send(Some(admitted_set(&[7, 8, 9]))).unwrap();
    wait_until(std::time::Duration::from_secs(5), async || {
        wallet
            .joined
            .lock()
            .unwrap()
            .contains(&readmitted_companion_id)
    })
    .await;
    assert_eq!(
        wallet
            .join_calls
            .lock()
            .unwrap()
            .iter()
            .filter(|id| id.as_str() == failed_id)
            .count(),
        1,
        "removal and re-admission must not retry in the same process"
    );

    task.shutdown().await.unwrap();

    // A replacement reconciler in the same process shares the attempt ledger.
    let (sender, receiver) = watch::channel(Some(admitted_set(&[7, 8, 9, 10])));
    let task = spawn_setup_payment_join_reconciler_with_attempts(
        wallet.clone(),
        receiver,
        attempted_federation_ids,
    );
    wait_until(std::time::Duration::from_secs(5), async || {
        wallet
            .joined
            .lock()
            .unwrap()
            .contains(&replacement_companion_id)
    })
    .await;
    assert_eq!(
        wallet
            .join_calls
            .lock()
            .unwrap()
            .iter()
            .filter(|id| id.as_str() == failed_id)
            .count(),
        1,
        "replacing the reconciler must not retry in the same process"
    );

    drop(sender);
    tokio::time::timeout(std::time::Duration::from_secs(5), task.join())
        .await
        .expect("reconciler exits when policy sender drops")
        .expect("reconciler task exits cleanly");

    // A fresh process-owned ledger models restart after stale tasks are gone.
    let (sender, receiver) = watch::channel(Some(admitted_set(&[7])));
    let task = spawn_setup_payment_join_reconciler_with_attempts(
        wallet.clone(),
        receiver,
        fresh_attempted_federation_ids(),
    );
    wait_until(std::time::Duration::from_secs(5), async || {
        wallet.joined.lock().unwrap().contains(&failed_id)
    })
    .await;
    assert_eq!(
        wallet
            .join_calls
            .lock()
            .unwrap()
            .iter()
            .filter(|id| id.as_str() == failed_id)
            .count(),
        2,
        "a fresh process ledger retries the failed join"
    );

    drop(sender);
    tokio::time::timeout(std::time::Duration::from_secs(5), task.join())
        .await
        .expect("reconciler exits when policy sender drops")
        .expect("reconciler task exits cleanly");
}

#[tokio::test]
async fn stalled_member_does_not_block_others_or_policy_replacement() {
    let wallet = Arc::new(RecordingWallet::default());
    let stalled_id = invite_for_id(11).0;
    let healthy_id = invite_for_id(12).0;
    let replacement_id = invite_for_id(13).0;
    wallet
        .pending_joins
        .lock()
        .unwrap()
        .insert(stalled_id.clone());

    let (sender, receiver) = watch::channel(Some(admitted_set(&[11, 12])));
    let task = spawn_setup_payment_join_reconciler_with_attempts(
        wallet.clone(),
        receiver,
        fresh_attempted_federation_ids(),
    );
    wait_until(std::time::Duration::from_secs(5), async || {
        wallet.joined.lock().unwrap().contains(&healthy_id)
    })
    .await;

    sender.send(Some(admitted_set(&[12, 13]))).unwrap();
    wait_until(std::time::Duration::from_secs(5), async || {
        wallet.cancelled_joins.lock().unwrap().contains(&stalled_id)
            && wallet.joined.lock().unwrap().contains(&replacement_id)
    })
    .await;

    drop(sender);
    tokio::time::timeout(std::time::Duration::from_secs(5), task.join())
        .await
        .expect("reconciler exits when policy sender drops")
        .expect("reconciler task exits cleanly");
}

#[tokio::test]
async fn rapid_full_set_replacement_never_exceeds_the_protocol_limit() {
    let wallet = Arc::new(RecordingWallet::default());
    let first: Vec<usize> = (100..116).collect();
    let intermediate: Vec<usize> = (200..216).collect();
    let replacement: Vec<usize> = (300..316).collect();
    wallet
        .pending_joins
        .lock()
        .unwrap()
        .extend(first.iter().map(|index| invite_for_id(*index).0));

    let (sender, receiver) = watch::channel(Some(admitted_set(&first)));
    let task = spawn_setup_payment_join_reconciler_with_attempts(
        wallet.clone(),
        receiver,
        fresh_attempted_federation_ids(),
    );
    wait_until(std::time::Duration::from_secs(5), async || {
        wallet.active_joins.load(Ordering::SeqCst) == SETUP_PAYMENT_FEDERATIONS_MAX_COUNT
    })
    .await;

    sender.send(Some(admitted_set(&intermediate))).unwrap();
    sender.send(Some(admitted_set(&replacement))).unwrap();
    let replacement_ids: BTreeSet<_> = replacement
        .iter()
        .map(|index| invite_for_id(*index).0)
        .collect();
    wait_until(std::time::Duration::from_secs(5), async || {
        replacement_ids.is_subset(&wallet.joined.lock().unwrap())
    })
    .await;
    assert_eq!(
        wallet.max_active_joins.load(Ordering::SeqCst),
        SETUP_PAYMENT_FEDERATIONS_MAX_COUNT
    );

    drop(sender);
    tokio::time::timeout(std::time::Duration::from_secs(5), task.join())
        .await
        .expect("reconciler exits when policy sender drops")
        .expect("reconciler task exits cleanly");
}

#[tokio::test]
async fn cancellation_cannot_reset_production_process_attempts() {
    let wallet = Arc::new(RecordingWallet::default());
    let pending_id = invite_for_id(11).0;
    let new_id = invite_for_id(12).0;
    wallet
        .pending_joins
        .lock()
        .unwrap()
        .insert(pending_id.clone());

    let (_sender, receiver) = watch::channel(Some(admitted_set(&[11])));
    let task = spawn_setup_payment_join_reconciler(wallet.clone(), receiver);
    wait_until(std::time::Duration::from_secs(5), async || {
        wallet.join_calls.lock().unwrap().contains(&pending_id)
    })
    .await;
    task.shutdown().await.unwrap();
    assert!(
        wallet.cancelled_joins.lock().unwrap().contains(&pending_id),
        "graceful shutdown must finish child cancellation before returning"
    );

    wallet.pending_joins.lock().unwrap().clear();
    let (sender, receiver) = watch::channel(Some(admitted_set(&[11, 12])));
    let task = spawn_setup_payment_join_reconciler(wallet.clone(), receiver);
    wait_until(std::time::Duration::from_secs(5), async || {
        wallet.joined.lock().unwrap().contains(&new_id)
    })
    .await;
    assert_eq!(
        wallet
            .join_calls
            .lock()
            .unwrap()
            .iter()
            .filter(|id| **id == pending_id)
            .count(),
        1,
        "replacement through the production wrapper must share pre-await attempts"
    );

    drop(sender);
    tokio::time::timeout(std::time::Duration::from_secs(5), task.join())
        .await
        .expect("reconciler exits when policy sender drops")
        .expect("reconciler task exits cleanly");
}
