use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use fedi_decentralized_service_manifold_guardian::{
    FederationId, LockedBlindedSignature, PaymentTerms, QuoteId, QuoteTerms, RefundIssuance,
    RefundTransaction,
};
use fedimint_core::PeerId;
use fedimint_core::invite_code::InviteCode as FedimintInviteCode;
use fedimint_core::util::SafeUrl;
use tokio::sync::watch;

use super::*;
use mg_core::wallet::{LockedPaymentPrepareError, Msats, NoWallet, VerifiedLockedPayment};

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

/// A second invite for federation `index`, naming a different guardian URL.
fn other_invite_for_id(index: usize) -> String {
    let federation_id: fedimint_core::config::FederationId = format!("{index:064x}")
        .parse()
        .expect("test federation ID is valid");
    FedimintInviteCode::new(
        SafeUrl::parse(&format!("https://other-{index}.example/")).expect("test URL is valid"),
        PeerId::from(0),
        federation_id,
        None,
    )
    .to_string()
}

fn admitted_set(indices: &[usize]) -> AdmittedSetupPaymentFederations {
    admitted_invites(
        indices
            .iter()
            .map(|index| invite_for_id(*index).1)
            .collect(),
    )
}

fn admitted_invites(invites: Vec<String>) -> AdmittedSetupPaymentFederations {
    let content = serde_json::json!({
        "version": 1,
        "fman_version": "0.1.0",
        "federations": invites,
        "telemetry_registration_url": "https://push.fedi.example/v1/telemetry/registrations",
    });
    AdmittedSetupPaymentFederations::parse(content.to_string().as_bytes())
        .expect("test set is admissible")
}

/// Join-recording wallet: joins succeed unless the invite's federation is
/// unreachable, fails its client open, or is configured to remain pending.
/// Successful joins become durable membership. Like the real wallet, a failed
/// open fences that federation for the rest of the process.
#[derive(Default)]
struct RecordingWallet {
    joined: std::sync::Mutex<BTreeSet<String>>,
    join_calls: std::sync::Mutex<Vec<String>>,
    unreachable: std::sync::Mutex<BTreeSet<String>>,
    unreachable_invites: std::sync::Mutex<BTreeSet<String>>,
    joined_with: std::sync::Mutex<Vec<String>>,
    failing_opens: std::sync::Mutex<BTreeSet<String>>,
    fenced: std::sync::Mutex<BTreeSet<String>>,
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

    async fn join(&self, invite_code: &str) -> Result<FederationId, JoinError> {
        let invite: FedimintInviteCode = invite_code.parse().map_err(JoinError::Retryable)?;
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
        if self.fenced.lock().unwrap().contains(&federation_id) {
            return Err(JoinError::RestartRequired(anyhow::anyhow!(
                "injected open fence"
            )));
        }
        if self.unreachable.lock().unwrap().contains(&federation_id)
            || self
                .unreachable_invites
                .lock()
                .unwrap()
                .contains(invite_code)
        {
            return Err(JoinError::Retryable(anyhow::anyhow!(
                "injected config download failure"
            )));
        }
        if self.failing_opens.lock().unwrap().contains(&federation_id) {
            self.fenced.lock().unwrap().insert(federation_id);
            return Err(JoinError::RestartRequired(anyhow::anyhow!(
                "injected open failure"
            )));
        }
        self.joined_with
            .lock()
            .unwrap()
            .push(invite_code.to_owned());
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

impl RecordingWallet {
    fn join_calls_for(&self, federation_id: &str) -> usize {
        self.join_calls
            .lock()
            .unwrap()
            .iter()
            .filter(|id| id.as_str() == federation_id)
            .count()
    }
}

#[tokio::test]
async fn joins_every_member_and_only_missing_members_on_update() {
    let wallet = Arc::new(RecordingWallet::default());
    let (sender, receiver) = watch::channel(Some(admitted_set(&[1, 2])));
    let task = spawn_setup_payment_join_reconciler(wallet.clone(), receiver);

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

/// Joins started at t=0 with retry waits of 5, 10, 20, 40 and then the 60 s
/// cap: attempts at 0, 5, 15, 35, 75, 135, 195, 255, 315, 375, 435, 495, 555.
const ATTEMPTS_IN_TEN_UNREACHABLE_MINUTES: usize = 13;

#[tokio::test(start_paused = true)]
async fn retryable_failures_back_off_and_join_once_reachable() {
    let wallet = Arc::new(RecordingWallet::default());
    let unreachable_id = invite_for_id(7).0;
    wallet
        .unreachable
        .lock()
        .unwrap()
        .insert(unreachable_id.clone());
    let (sender, receiver) = watch::channel(Some(admitted_set(&[7])));
    let task = spawn_setup_payment_join_reconciler(wallet.clone(), receiver);

    tokio::time::sleep(std::time::Duration::from_secs(600)).await;
    assert_eq!(
        wallet.join_calls_for(&unreachable_id),
        ATTEMPTS_IN_TEN_UNREACHABLE_MINUTES,
        "retries must double from 5 s up to a 60 s cap"
    );

    // Reachable again: the next scheduled retry joins without a restart or
    // a policy change.
    wallet.unreachable.lock().unwrap().clear();
    tokio::time::sleep(std::time::Duration::from_secs(60)).await;
    assert!(wallet.joined.lock().unwrap().contains(&unreachable_id));
    let calls = wallet.join_calls_for(&unreachable_id);
    assert_eq!(calls, ATTEMPTS_IN_TEN_UNREACHABLE_MINUTES + 1);
    tokio::time::sleep(std::time::Duration::from_secs(600)).await;
    assert_eq!(
        wallet.join_calls_for(&unreachable_id),
        calls,
        "a joined federation is not attempted again"
    );

    drop(sender);
    tokio::time::timeout(std::time::Duration::from_secs(5), task.join())
        .await
        .expect("reconciler exits when policy sender drops")
        .expect("reconciler task exits cleanly");
}

#[tokio::test(start_paused = true)]
async fn refused_open_is_not_retried() {
    let wallet = Arc::new(RecordingWallet::default());
    let fenced_id = invite_for_id(7).0;
    wallet
        .failing_opens
        .lock()
        .unwrap()
        .insert(fenced_id.clone());
    let (sender, receiver) = watch::channel(Some(admitted_set(&[7])));
    let task = spawn_setup_payment_join_reconciler(wallet.clone(), receiver);

    tokio::time::sleep(std::time::Duration::from_secs(600)).await;
    assert_eq!(
        wallet.join_calls_for(&fenced_id),
        1,
        "a join the wallet fenced must wait for a restart"
    );

    // Later policy updates keep it waiting too; other members still join.
    let other_id = invite_for_id(8).0;
    sender.send(Some(admitted_set(&[7, 8]))).unwrap();
    wait_until(std::time::Duration::from_secs(5), async || {
        wallet.joined.lock().unwrap().contains(&other_id)
    })
    .await;
    tokio::time::sleep(std::time::Duration::from_secs(600)).await;
    assert_eq!(wallet.join_calls_for(&fenced_id), 1);

    // The fence is per federation: a replaced invite does not lift it.
    sender
        .send(Some(admitted_invites(vec![
            other_invite_for_id(7),
            invite_for_id(8).1,
        ])))
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_secs(600)).await;
    assert_eq!(wallet.join_calls_for(&fenced_id), 1);
    assert!(!wallet.joined.lock().unwrap().contains(&fenced_id));

    drop(sender);
    tokio::time::timeout(std::time::Duration::from_secs(5), task.join())
        .await
        .expect("reconciler exits when policy sender drops")
        .expect("reconciler task exits cleanly");
}

#[tokio::test(start_paused = true)]
async fn replaced_invite_restarts_a_retrying_join() {
    let wallet = Arc::new(RecordingWallet::default());
    let federation_id = invite_for_id(7).0;
    let broken_invite = other_invite_for_id(7);
    let working_invite = invite_for_id(7).1;
    wallet
        .unreachable_invites
        .lock()
        .unwrap()
        .insert(broken_invite.clone());
    let (sender, receiver) = watch::channel(Some(admitted_invites(vec![broken_invite])));
    let task = spawn_setup_payment_join_reconciler(wallet.clone(), receiver);

    // Let the broken invite reach a long backoff.
    tokio::time::sleep(std::time::Duration::from_secs(300)).await;
    let failed_calls = wallet.join_calls_for(&federation_id);
    assert!(failed_calls > 1);
    assert!(wallet.joined.lock().unwrap().is_empty());

    // Same federation, new invite: joined at once, not after the backoff.
    sender
        .send(Some(admitted_invites(vec![working_invite.clone()])))
        .unwrap();
    wait_until(std::time::Duration::from_secs(1), async || {
        wallet.joined.lock().unwrap().contains(&federation_id)
    })
    .await;
    assert_eq!(*wallet.joined_with.lock().unwrap(), vec![working_invite]);
    assert_eq!(wallet.join_calls_for(&federation_id), failed_calls + 1);

    drop(sender);
    tokio::time::timeout(std::time::Duration::from_secs(5), task.join())
        .await
        .expect("reconciler exits when policy sender drops")
        .expect("reconciler task exits cleanly");
}

#[tokio::test]
async fn replaced_invite_does_not_cancel_an_attempt_in_flight() {
    let wallet = Arc::new(RecordingWallet::default());
    let federation_id = invite_for_id(7).0;
    wallet
        .pending_joins
        .lock()
        .unwrap()
        .insert(federation_id.clone());
    let (sender, receiver) = watch::channel(Some(admitted_set(&[7])));
    let task = spawn_setup_payment_join_reconciler(wallet.clone(), receiver);
    wait_until(std::time::Duration::from_secs(5), async || {
        wallet.join_calls_for(&federation_id) == 1
    })
    .await;

    // Cancelling an attempt that may be opening the client would fence the
    // wallet until restart.
    sender
        .send(Some(admitted_invites(vec![other_invite_for_id(7)])))
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert!(wallet.cancelled_joins.lock().unwrap().is_empty());
    assert_eq!(wallet.join_calls_for(&federation_id), 1);

    task.shutdown().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn removal_stops_retrying_and_readmission_retries_at_once() {
    let wallet = Arc::new(RecordingWallet::default());
    let unreachable_id = invite_for_id(7).0;
    let replacement_id = invite_for_id(8).0;
    wallet
        .unreachable
        .lock()
        .unwrap()
        .insert(unreachable_id.clone());
    let (sender, receiver) = watch::channel(Some(admitted_set(&[7])));
    let task = spawn_setup_payment_join_reconciler(wallet.clone(), receiver);
    wait_until(std::time::Duration::from_secs(5), async || {
        wallet.join_calls_for(&unreachable_id) == 1
    })
    .await;

    sender.send(Some(admitted_set(&[8]))).unwrap();
    tokio::time::sleep(std::time::Duration::from_secs(600)).await;
    assert!(wallet.joined.lock().unwrap().contains(&replacement_id));
    assert_eq!(
        wallet.join_calls_for(&unreachable_id),
        1,
        "a removed federation is not retried"
    );

    // Re-admission starts a fresh attempt immediately, not after the backoff
    // the removed attempt had reached.
    wallet.unreachable.lock().unwrap().clear();
    sender.send(Some(admitted_set(&[7, 8]))).unwrap();
    wait_until(std::time::Duration::from_secs(1), async || {
        wallet.joined.lock().unwrap().contains(&unreachable_id)
    })
    .await;
    assert_eq!(wallet.join_calls_for(&unreachable_id), 2);

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
    let task = spawn_setup_payment_join_reconciler(wallet.clone(), receiver);
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
    let task = spawn_setup_payment_join_reconciler(wallet.clone(), receiver);
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
async fn shutdown_cancels_pending_joins_before_returning() {
    let wallet = Arc::new(RecordingWallet::default());
    let pending_id = invite_for_id(11).0;
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
}
