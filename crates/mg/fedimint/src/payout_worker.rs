//! Durable caller-idempotent payout orchestration owned beside the native wallet.

use anyhow::Context as _;
use fedi_decentralized_service_manifold_guardian::{
    FederationId, InviteCode as WireInviteCode, SeatId,
};
use fedimint_client::ClientHandleArc;
use fedimint_core::invite_code::InviteCode;
use mg_core::db::Db;
use mg_core::guardian_fee::GuardianFeeAccountKey;
use mg_core::payout_wire::{
    PayoutJobStatusWire, PayoutJobWire, PayoutsWire, WalletDrainStatusWire,
};
use mg_core::wallet::{EcashPayoutWorker, PayoutRail, PayoutRequestId};
use std::sync::Arc;

use crate::payout_job::{Payout, PayoutJob, PayoutMethod, PayoutScope};
use crate::payout_job_status::{PayoutJobStatus, PayoutNativeStatus};
use crate::payout_operation_id::PayoutOperationId;
use crate::wallet_drain::WalletDrainStatus;
use crate::{ClientScope, OpenWait, Wallet};

/// How many of the newest payouts a list holds. Every listed job opens its
/// wallet to read its state.
const RECENT_PAYOUTS: u32 = 20;

pub(crate) struct PayoutWorker {
    wallet: Option<Arc<Wallet>>,
    db: Db,
    native: Arc<dyn PayoutNative>,
    guardian_key: Arc<dyn Fn(&SeatId) -> GuardianFeeAccountKey + Send + Sync>,
}
impl PayoutWorker {
    pub(crate) fn start(
        wallet: Arc<Wallet>,
        db: Db,
        guardian_key: Arc<dyn Fn(&SeatId) -> GuardianFeeAccountKey + Send + Sync>,
    ) -> Arc<Self> {
        Arc::new(Self {
            native: Arc::new(WalletPayoutNative {
                wallet: wallet.clone(),
                db: db.clone(),
                guardian_key: guardian_key.clone(),
            }),
            wallet: Some(wallet),
            db,
            guardian_key,
        })
    }
}

/// Wallet-side effects required by the durable payout orchestrator.
///
/// The production adapter owns Fedimint client opening and the wallet's
/// payout fence, which serializes every start and reclaim of one wallet.
#[async_trait::async_trait]
trait PayoutNative: Send + Sync {
    /// Start the job's native payout under the operation id the ledger names
    /// for it, or find the one already there.
    async fn start(&self, job: &PayoutJob) -> anyhow::Result<Payout>;

    /// Find the native payout the ledger names for the job, without starting
    /// one. Unless `wait`, a wallet that another task is opening, or that was
    /// never joined, is an error rather than a wait.
    async fn find(&self, job: &PayoutJob, wait: bool) -> anyhow::Result<Option<Payout>>;

    /// Read or await one already committed native payout. For an ecash payout,
    /// awaiting means awaiting a submitted reclaim. A read that does not wait
    /// opens the wallet as `find` does.
    async fn observe(
        &self,
        job: &PayoutJob,
        id: PayoutOperationId,
        wait: bool,
    ) -> anyhow::Result<PayoutNativeStatus>;

    /// Submit, or find already submitted, the reclaim of one committed ecash
    /// payout's notes.
    async fn reclaim(&self, job: &PayoutJob, id: PayoutOperationId) -> anyhow::Result<()>;
}

/// Production Fedimint implementation of [`PayoutNative`].
struct WalletPayoutNative {
    /// Shared wallet that opens the client for one persisted payout scope.
    wallet: Arc<Wallet>,
    /// The payout ledger, which records a Lightning attempt before paying it.
    db: Db,
    /// Mnemonic-derived key lookup for a guardian-fee wallet scope.
    guardian_key: Arc<dyn Fn(&SeatId) -> GuardianFeeAccountKey + Send + Sync>,
}

#[async_trait::async_trait]
impl EcashPayoutWorker for PayoutWorker {
    async fn sweep_payment_fees(
        &self,
        federation_id: &FederationId,
        request_id: &PayoutRequestId,
        rail: PayoutRail,
    ) -> anyhow::Result<PayoutJobWire> {
        let scope = payment_scope(federation_id);
        Ok(self.start_job(request_id, scope, rail).await?.to_wire())
    }
    async fn sweep_guardian_fees(
        &self,
        invite: &WireInviteCode,
        seat_id: &SeatId,
        request_id: &PayoutRequestId,
        rail: PayoutRail,
    ) -> anyhow::Result<PayoutJobWire> {
        let scope = guardian_scope(invite, seat_id)?;
        Ok(self.start_job(request_id, scope, rail).await?.to_wire())
    }
    async fn resume_guardian_sweep(
        &self,
        seat_id: &SeatId,
        request_id: &PayoutRequestId,
        rail: PayoutRail,
    ) -> anyhow::Result<Option<PayoutJobWire>> {
        let Some(job) = stored_guardian_job(&self.db, seat_id, request_id).await? else {
            return Ok(None);
        };
        let job = self.start_job(request_id, job.scope.clone(), rail).await?;
        Ok(Some(job.to_wire()))
    }
    async fn confirm_ecash_payout(
        &self,
        request_id: &PayoutRequestId,
    ) -> anyhow::Result<PayoutJobWire> {
        Ok(crate::payout_store::confirm(&self.db, request_id)
            .await?
            .to_wire())
    }
    async fn cancel_ecash_payout(
        &self,
        request_id: &PayoutRequestId,
    ) -> anyhow::Result<PayoutJobStatusWire> {
        Ok(self.cancel(request_id).await?.to_wire(true))
    }
    async fn list_payouts(&self) -> anyhow::Result<PayoutsWire> {
        Ok(PayoutsWire {
            payouts: self
                .list()
                .await?
                .iter()
                .map(|status| status.to_wire(false))
                .collect(),
        })
    }
    async fn payout_status(
        &self,
        request_id: &PayoutRequestId,
    ) -> anyhow::Result<PayoutJobStatusWire> {
        Ok(self.status(request_id, false).await?.to_wire(true))
    }
    async fn await_payout(
        &self,
        request_id: &PayoutRequestId,
    ) -> anyhow::Result<PayoutJobStatusWire> {
        Ok(self.status(request_id, true).await?.to_wire(true))
    }
    async fn payment_drain_status(&self, federation_id: &FederationId) -> WalletDrainStatusWire {
        let status = match (self.wallet.as_ref(), federation_id.0.parse()) {
            (Some(wallet), Ok(id)) => match wallet.client_with(id, OpenWait::Refuse).await {
                Ok(c) => {
                    let scope = payment_scope(federation_id);
                    let open = self.holds_open_token(&c, &scope).await;
                    crate::drain_status::wallet_drain_status(&c)
                        .await
                        .with_open_ecash_payout(open)
                }
                Err(crate::WalletError::ClientOpening {
                    recovering: true, ..
                }) => return WalletDrainStatusWire::recovering(),
                Err(_) => WalletDrainStatus::unavailable(),
            },
            (_, Err(_)) | (None, Ok(_)) => WalletDrainStatus::unavailable(),
        };
        status.to_wire()
    }
    async fn guardian_drain_status(
        &self,
        invite: &WireInviteCode,
        seat_id: &SeatId,
    ) -> anyhow::Result<WalletDrainStatusWire> {
        let Some(wallet) = &self.wallet else {
            return Ok(WalletDrainStatus::unavailable().to_wire());
        };
        let scope = guardian_scope(invite, seat_id)?;
        validate_scope(&scope)?;
        let PayoutScope::GuardianFee { invite_code, .. } = &scope else {
            unreachable!("guardian_scope builds a guardian scope")
        };
        let key = (self.guardian_key)(seat_id);
        match wallet
            .guardian_fee_client(invite_code, seat_id, &key, OpenWait::Refuse)
            .await
        {
            Ok(c) => {
                let open = self.holds_open_token(&c, &scope).await;
                Ok(crate::drain_status::wallet_drain_status(&c)
                    .await
                    .with_open_ecash_payout(open)
                    .to_wire())
            }
            Err(error)
                if matches!(
                    error.downcast_ref(),
                    Some(crate::WalletError::ClientOpening {
                        recovering: true,
                        ..
                    })
                ) =>
            {
                Ok(WalletDrainStatusWire::recovering())
            }
            Err(_) => Ok(WalletDrainStatus::unavailable().to_wire()),
        }
    }
}

impl PayoutWorker {
    /// Whether the scope's wallet holds an ecash payout whose token may still
    /// be unredeemed: issued and not marked redeemed, or being reclaimed. That
    /// token exists only in this wallet's database, so the wallet is not
    /// drained while it is open.
    async fn holds_open_token(
        &self,
        client: &ClientHandleArc,
        scope: &PayoutScope,
    ) -> anyhow::Result<bool> {
        for job in crate::payout_store::unconfirmed_ecash(&self.db).await? {
            if !same_wallet(&job.scope, scope) {
                continue;
            }
            let id = match &job.operation {
                Some(operation) => operation.operation_id.clone(),
                None => PayoutOperationId::ecash(&job.request_id),
            };
            if job.operation.is_none()
                && crate::payout_ecash::find_ecash_payout(client, &id)
                    .await?
                    .is_none()
            {
                continue;
            }
            if crate::payout_ecash::ecash_payout_state(client, &id, false)
                .await?
                .is_open()
            {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

/// Whether two scopes name the same wallet. A guardian-fee scope's invite can
/// be written differently for one federation, so it is not compared.
fn same_wallet(left: &PayoutScope, right: &PayoutScope) -> bool {
    match (left, right) {
        (
            PayoutScope::PaymentFederation { federation_id: a },
            PayoutScope::PaymentFederation { federation_id: b },
        ) => a == b,
        (
            PayoutScope::GuardianFee {
                federation_id: a,
                seat_id: sa,
                ..
            },
            PayoutScope::GuardianFee {
                federation_id: b,
                seat_id: sb,
                ..
            },
        ) => a == b && sa == sb,
        _ => false,
    }
}

fn payment_scope(federation_id: &FederationId) -> PayoutScope {
    PayoutScope::PaymentFederation {
        federation_id: federation_id.clone(),
    }
}

fn guardian_scope(invite: &WireInviteCode, seat_id: &SeatId) -> anyhow::Result<PayoutScope> {
    let invite: InviteCode = invite
        .0
        .parse()
        .context("invalid guardian federation invite")?;
    Ok(PayoutScope::GuardianFee {
        federation_id: FederationId(invite.federation_id().to_string()),
        seat_id: seat_id.clone(),
        invite_code: invite,
    })
}

async fn stored_guardian_job(
    db: &Db,
    seat_id: &SeatId,
    request_id: &PayoutRequestId,
) -> anyhow::Result<Option<PayoutJob>> {
    let Some(job) = crate::payout_store::get(db, request_id).await? else {
        return Ok(None);
    };
    match &job.scope {
        PayoutScope::GuardianFee {
            federation_id,
            seat_id: stored_seat_id,
            invite_code,
        } if stored_seat_id == seat_id => {
            anyhow::ensure!(
                federation_id.0 == invite_code.federation_id().to_string(),
                "payout request wallet scope no longer matches the seat federation"
            );
            Ok(Some(job))
        }
        _ => anyhow::bail!("payout request id is already bound to a different scope"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use tokio::sync::Mutex;

    use crate::payout_ecash::{EcashPayoutState, EcashToken};
    use crate::wallet_drain::OutgoingOperation;

    /// Where the notes of one fake ecash spend are.
    #[derive(Default)]
    struct FakeNotes {
        /// The recipient spent them.
        redeemed: bool,
        /// This wallet submitted their reissue.
        reclaimed: bool,
        /// That reissue has not reached a terminal state.
        reclaim_running: bool,
    }

    #[derive(Default)]
    struct FakeNative {
        /// The wallet's payout fence.
        fence: Mutex<()>,
        /// Native payouts by the operation id the ledger names for a request.
        payouts: Mutex<HashMap<PayoutRequestId, Payout>>,
        notes: Mutex<HashMap<PayoutOperationId, FakeNotes>>,
        lose_next_response: AtomicBool,
        refuse_next_start: AtomicBool,
        /// A job whose wallet cannot be read.
        unreadable: Mutex<Option<PayoutRequestId>>,
        starts: AtomicUsize,
        statuses: AtomicUsize,
        awaits: AtomicUsize,
        reclaims: AtomicUsize,
    }

    fn lightning(destination: &str) -> PayoutMethod {
        PayoutMethod::Lightning {
            destination: destination.to_owned(),
        }
    }

    #[async_trait::async_trait]
    impl PayoutNative for FakeNative {
        async fn start(&self, job: &PayoutJob) -> anyhow::Result<Payout> {
            let _fence = self.fence.lock().await;
            if let Some(payout) = self.payouts.lock().await.get(&job.request_id).cloned() {
                return Ok(payout);
            }
            if self.refuse_next_start.swap(false, Ordering::SeqCst) {
                anyhow::bail!("simulated start refusal: no gateway");
            }
            self.starts.fetch_add(1, Ordering::SeqCst);
            // A second caller reaches this yield before the first writes
            // unless the fence holds across the lookup and the start.
            tokio::task::yield_now().await;
            let started = self.starts.load(Ordering::SeqCst);
            let payout = Payout {
                operation_id: PayoutOperationId::parse(&format!("{started:064x}"))?,
                amount_msat: 42,
            };
            if job.method == PayoutMethod::Ecash {
                self.notes
                    .lock()
                    .await
                    .insert(payout.operation_id.clone(), FakeNotes::default());
            }
            self.payouts
                .lock()
                .await
                .insert(job.request_id.clone(), payout.clone());
            if self.lose_next_response.swap(false, Ordering::SeqCst) {
                anyhow::bail!("simulated native response loss");
            }
            Ok(payout)
        }

        async fn find(&self, job: &PayoutJob, _wait: bool) -> anyhow::Result<Option<Payout>> {
            Ok(self.payouts.lock().await.get(&job.request_id).cloned())
        }

        async fn observe(
            &self,
            job: &PayoutJob,
            id: PayoutOperationId,
            wait: bool,
        ) -> anyhow::Result<PayoutNativeStatus> {
            if wait {
                self.awaits.fetch_add(1, Ordering::SeqCst);
            } else {
                self.statuses.fetch_add(1, Ordering::SeqCst);
            }
            if self.unreadable.lock().await.as_ref() == Some(&job.request_id) {
                anyhow::bail!("simulated wallet open failure");
            }
            if job.method == PayoutMethod::Ecash {
                let notes = self.notes.lock().await;
                let notes = notes.get(&id).context("observed an uncommitted spend")?;
                let state = match (notes.reclaimed, notes.redeemed) {
                    (false, _) => EcashPayoutState::Issued {
                        token: EcashToken::test(&format!("token-for-{id}")),
                    },
                    (true, _) if !wait && notes.reclaim_running => EcashPayoutState::Reclaiming,
                    (true, false) => EcashPayoutState::Reclaimed,
                    (true, true) => EcashPayoutState::AlreadySpent {
                        token: EcashToken::test(&format!("token-for-{id}")),
                    },
                };
                return Ok(PayoutNativeStatus::Ecash(Some(state)));
            }
            Ok(PayoutNativeStatus::Lightning(Some(OutgoingOperation::new(
                id,
                crate::wallet_drain::OutgoingRail::Lnv2,
                crate::wallet_drain::OutgoingState::Succeeded,
                42,
                42,
                false,
            ))))
        }

        async fn reclaim(&self, job: &PayoutJob, id: PayoutOperationId) -> anyhow::Result<()> {
            assert_eq!(job.method, PayoutMethod::Ecash);
            self.reclaims.fetch_add(1, Ordering::SeqCst);
            let mut notes = self.notes.lock().await;
            let notes = notes
                .get_mut(&id)
                .context("reclaimed an uncommitted spend")?;
            notes.reclaimed = true;
            Ok(())
        }
    }

    fn worker(db: Db, native: Arc<FakeNative>) -> PayoutWorker {
        PayoutWorker {
            wallet: None,
            db,
            native,
            guardian_key: Arc::new(|_| GuardianFeeAccountKey::from_secret_bytes(&[1; 32])),
        }
    }

    fn invite(federation: &str) -> InviteCode {
        InviteCode::new(
            fedimint_core::util::SafeUrl::parse("https://guardian.example").unwrap(),
            fedimint_core::PeerId::from(0),
            federation.parse().unwrap(),
            None,
        )
    }

    fn guardian_scope(seat_id: &SeatId, federation: &str) -> PayoutScope {
        let invite_code = invite(federation);
        PayoutScope::GuardianFee {
            federation_id: FederationId(invite_code.federation_id().to_string()),
            seat_id: seat_id.clone(),
            invite_code,
        }
    }

    async fn set_destination(db: &Db, destination: &str) {
        sqlx::query("UPDATE payout_settings SET destination = ? WHERE id = 1")
            .bind(destination)
            .execute(db.pool())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn guardian_stored_scope_requires_matching_invite_federation() {
        let temp = tempfile::tempdir().unwrap();
        let db = Db::open(temp.path()).await.unwrap();
        let seat_id = SeatId::new("01".repeat(32)).unwrap();
        let request_id = PayoutRequestId::parse("guardian-stored-scope").unwrap();
        let scope = guardian_scope(&seat_id, &"02".repeat(32));
        crate::payout_store::create(&db, &request_id, &scope, &lightning("operator@example.com"))
            .await
            .unwrap();
        assert_eq!(
            stored_guardian_job(&db, &seat_id, &request_id)
                .await
                .unwrap()
                .unwrap()
                .scope,
            scope
        );

        let mismatch = PayoutRequestId::parse("guardian-mismatched-scope").unwrap();
        let PayoutScope::GuardianFee {
            seat_id,
            invite_code,
            ..
        } = guardian_scope(&seat_id, &"02".repeat(32))
        else {
            unreachable!()
        };
        let malformed = PayoutScope::GuardianFee {
            federation_id: FederationId("03".repeat(32)),
            seat_id,
            invite_code,
        };
        crate::payout_store::create(
            &db,
            &mismatch,
            &malformed,
            &lightning("operator@example.com"),
        )
        .await
        .unwrap();
        assert!(
            stored_guardian_job(&db, &SeatId::new("01".repeat(32)).unwrap(), &mismatch)
                .await
                .unwrap_err()
                .to_string()
                .contains("no longer matches")
        );
    }

    #[tokio::test]
    async fn lost_response_recovers_across_reopen_with_one_native_start() {
        let temp = tempfile::tempdir().unwrap();
        let db = Db::open(temp.path()).await.unwrap();
        set_destination(&db, "first@example.com").await;
        let native = Arc::new(FakeNative {
            lose_next_response: AtomicBool::new(true),
            ..Default::default()
        });
        let request_id = PayoutRequestId::parse("lost-response").unwrap();
        let scope = PayoutScope::PaymentFederation {
            federation_id: FederationId("payment-federation".into()),
        };
        assert!(
            worker(db.clone(), native.clone())
                .start_job(&request_id, scope.clone(), PayoutRail::Lightning)
                .await
                .unwrap_err()
                .to_string()
                .contains("response loss")
        );
        drop(db);

        let reopened = Db::open(temp.path()).await.unwrap();
        set_destination(&reopened, "changed@example.com").await;
        let job = worker(reopened, native.clone())
            .start_job(&request_id, scope, PayoutRail::Lightning)
            .await
            .unwrap();
        assert_eq!(native.starts.load(Ordering::SeqCst), 1);
        assert_eq!(job.method, lightning("first@example.com"));
        assert!(job.operation.is_some());
    }

    #[tokio::test]
    async fn concurrent_retries_start_one_native_payout() {
        let temp = tempfile::tempdir().unwrap();
        let db = Db::open(temp.path()).await.unwrap();
        set_destination(&db, "operator@example.com").await;
        let native = Arc::new(FakeNative::default());
        let worker = Arc::new(worker(db, native.clone()));
        let request_id = PayoutRequestId::parse("concurrent-request").unwrap();
        let scope = PayoutScope::PaymentFederation {
            federation_id: FederationId("payment-federation".into()),
        };
        let (left, right) = tokio::join!(
            worker.start_job(&request_id, scope.clone(), PayoutRail::Lightning),
            worker.start_job(&request_id, scope, PayoutRail::Lightning),
        );
        assert_eq!(left.unwrap(), right.unwrap());
        assert_eq!(native.starts.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn pending_status_has_no_start_authority() {
        let temp = tempfile::tempdir().unwrap();
        let db = Db::open(temp.path()).await.unwrap();
        let request_id = PayoutRequestId::parse("status-only").unwrap();
        let scope = PayoutScope::PaymentFederation {
            federation_id: FederationId("payment-federation".into()),
        };
        crate::payout_store::create(&db, &request_id, &scope, &lightning("operator@example.com"))
            .await
            .unwrap();
        let native = Arc::new(FakeNative::default());
        let status = worker(db, native.clone())
            .status(&request_id, false)
            .await
            .unwrap();
        assert!(status.job.operation.is_none());
        assert!(matches!(status.native, PayoutNativeStatus::Lightning(None)));
        assert_eq!(native.starts.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn guardian_status_and_await_replay_without_a_live_seat() {
        let temp = tempfile::tempdir().unwrap();
        let db = Db::open(temp.path()).await.unwrap();
        let seat_id = SeatId::new("71".repeat(32)).unwrap();
        let request_id = PayoutRequestId::parse("guardian-lost-response").unwrap();
        let scope = guardian_scope(&seat_id, &"02".repeat(32));
        crate::payout_store::create(&db, &request_id, &scope, &lightning("operator@example.com"))
            .await
            .unwrap();
        let native = Arc::new(FakeNative {
            lose_next_response: AtomicBool::new(true),
            ..Default::default()
        });
        let current = worker(db.clone(), native.clone());
        assert!(
            current
                .start_job(&request_id, scope, PayoutRail::Lightning)
                .await
                .unwrap_err()
                .to_string()
                .contains("response loss")
        );
        drop(current);
        drop(db);

        let reopened = worker(Db::open(temp.path()).await.unwrap(), native.clone());
        let status = reopened.status(&request_id, false).await.unwrap();
        assert!(matches!(
            status.native,
            PayoutNativeStatus::Lightning(Some(_))
        ));
        let awaited = reopened.status(&request_id, true).await.unwrap();
        assert!(matches!(
            awaited.native,
            PayoutNativeStatus::Lightning(Some(_))
        ));
        assert_eq!(native.starts.load(Ordering::SeqCst), 1);
        assert_eq!(native.statuses.load(Ordering::SeqCst), 1);
        assert_eq!(native.awaits.load(Ordering::SeqCst), 1);
    }

    fn ecash_request(name: &str) -> (PayoutRequestId, PayoutScope) {
        (
            PayoutRequestId::parse(name).unwrap(),
            PayoutScope::PaymentFederation {
                federation_id: FederationId("payment-federation".into()),
            },
        )
    }

    /// The listed ecash payouts still waiting on the operator: issued and
    /// not marked redeemed, or reclaiming.
    async fn open_payouts(worker: &PayoutWorker) -> Vec<PayoutJobStatus> {
        worker
            .list()
            .await
            .unwrap()
            .into_iter()
            .filter(|s| {
                matches!(&s.native, PayoutNativeStatus::Ecash(Some(state)) if state.is_open())
                    && s.job
                        .operation
                        .as_ref()
                        .is_some_and(|op| op.confirmed_at_ms.is_none())
            })
            .collect()
    }

    fn confirmed_at(job: &PayoutJob) -> Option<u64> {
        job.operation.as_ref().and_then(|op| op.confirmed_at_ms)
    }

    fn ecash_state(status: &PayoutJobStatus) -> &EcashPayoutState {
        match &status.native {
            PayoutNativeStatus::Ecash(Some(state)) => state,
            other => panic!("expected a committed ecash payout, got {other:?}"),
        }
    }

    /// Process death after the ledger row and before any wallet work leaves a
    /// row and no spend. Nothing is listed, and a retry under the same id
    /// spends once.
    #[tokio::test]
    async fn ecash_crash_before_spend_is_retried_under_the_same_request() {
        let temp = tempfile::tempdir().unwrap();
        let db = Db::open(temp.path()).await.unwrap();
        let (request_id, scope) = ecash_request("ecash-crash-before-spend");
        crate::payout_store::create(&db, &request_id, &scope, &PayoutMethod::Ecash)
            .await
            .unwrap();
        drop(db);

        let native = Arc::new(FakeNative::default());
        let reopened = worker(Db::open(temp.path()).await.unwrap(), native.clone());
        assert!(open_payouts(&reopened).await.is_empty());
        let pending = reopened.status(&request_id, false).await.unwrap();
        assert!(matches!(pending.native, PayoutNativeStatus::Ecash(None)));
        assert_eq!(native.starts.load(Ordering::SeqCst), 0);

        reopened
            .start_job(&request_id, scope.clone(), PayoutRail::Ecash)
            .await
            .unwrap();
        reopened
            .start_job(&request_id, scope, PayoutRail::Ecash)
            .await
            .unwrap();
        assert_eq!(native.starts.load(Ordering::SeqCst), 1);
        let open = open_payouts(&reopened).await;
        assert_eq!(open.len(), 1);
        assert!(matches!(
            ecash_state(&open[0]),
            EcashPayoutState::Issued { .. }
        ));
    }

    /// Process death after the spend committed and before the ledger linked
    /// it: the token is listed after restart, and a retry returns that spend
    /// rather than spending again.
    #[tokio::test]
    async fn ecash_crash_after_spend_lists_the_token_and_never_spends_twice() {
        let temp = tempfile::tempdir().unwrap();
        let db = Db::open(temp.path()).await.unwrap();
        let native = Arc::new(FakeNative {
            lose_next_response: AtomicBool::new(true),
            ..Default::default()
        });
        let (request_id, scope) = ecash_request("ecash-crash-after-spend");
        assert!(
            worker(db.clone(), native.clone())
                .start_job(&request_id, scope.clone(), PayoutRail::Ecash)
                .await
                .is_err()
        );
        assert!(
            crate::payout_store::get(&db, &request_id)
                .await
                .unwrap()
                .unwrap()
                .operation
                .is_none()
        );
        drop(db);

        let reopened = worker(Db::open(temp.path()).await.unwrap(), native.clone());
        let open = open_payouts(&reopened).await;
        assert_eq!(open.len(), 1);
        let committed = open[0].job.operation.clone().unwrap();
        assert!(matches!(
            ecash_state(&open[0]),
            EcashPayoutState::Issued { .. }
        ));
        let retried = reopened
            .start_job(&request_id, scope.clone(), PayoutRail::Ecash)
            .await
            .unwrap();
        assert_eq!(retried.operation.unwrap(), committed);
        assert_eq!(native.starts.load(Ordering::SeqCst), 1);
        // The id stays an ecash request: it cannot become a Lightning sweep.
        assert!(
            reopened
                .start_job(&request_id, scope, PayoutRail::Lightning)
                .await
                .unwrap_err()
                .to_string()
                .contains("different payout method")
        );
    }

    #[tokio::test]
    async fn concurrent_ecash_retries_spend_once() {
        let temp = tempfile::tempdir().unwrap();
        let native = Arc::new(FakeNative::default());
        let worker = Arc::new(worker(Db::open(temp.path()).await.unwrap(), native.clone()));
        let (request_id, scope) = ecash_request("ecash-concurrent");
        let (left, right) = tokio::join!(
            worker.start_job(&request_id, scope.clone(), PayoutRail::Ecash),
            worker.start_job(&request_id, scope, PayoutRail::Ecash),
        );
        assert_eq!(left.unwrap(), right.unwrap());
        assert_eq!(native.starts.load(Ordering::SeqCst), 1);
    }

    /// A cancel before the recipient redeems takes the notes back; repeating
    /// it resumes the same reclaim, and the payout leaves the open list.
    #[tokio::test]
    async fn ecash_cancel_before_redemption_reclaims_and_closes_the_payout() {
        let temp = tempfile::tempdir().unwrap();
        let native = Arc::new(FakeNative::default());
        let worker = worker(Db::open(temp.path()).await.unwrap(), native.clone());
        let (request_id, scope) = ecash_request("ecash-cancel-before");
        worker
            .start_job(&request_id, scope, PayoutRail::Ecash)
            .await
            .unwrap();

        let cancelled = worker.cancel(&request_id).await.unwrap();
        assert_eq!(ecash_state(&cancelled), &EcashPayoutState::Reclaimed);
        assert_eq!(
            ecash_state(&worker.cancel(&request_id).await.unwrap()),
            &EcashPayoutState::Reclaimed
        );
        assert!(open_payouts(&worker).await.is_empty());
        // A confirmation afterwards only records the operator's word; the
        // wallet still reads reclaimed.
        worker.confirm_ecash_payout(&request_id).await.unwrap();
        assert_eq!(
            ecash_state(&worker.status(&request_id, false).await.unwrap()),
            &EcashPayoutState::Reclaimed
        );
    }

    /// A cancel after the recipient redeemed reports the notes already spent.
    #[tokio::test]
    async fn ecash_cancel_after_redemption_reports_already_spent() {
        let temp = tempfile::tempdir().unwrap();
        let native = Arc::new(FakeNative::default());
        let worker = worker(Db::open(temp.path()).await.unwrap(), native.clone());
        let (request_id, scope) = ecash_request("ecash-cancel-after");
        let job = worker
            .start_job(&request_id, scope, PayoutRail::Ecash)
            .await
            .unwrap();
        native
            .notes
            .lock()
            .await
            .get_mut(&job.operation.unwrap().operation_id)
            .unwrap()
            .redeemed = true;

        assert!(matches!(
            ecash_state(&worker.cancel(&request_id).await.unwrap()),
            EcashPayoutState::AlreadySpent { .. }
        ));
        assert!(open_payouts(&worker).await.is_empty());
    }

    /// While the federation has not answered a reclaim, the payout stays open
    /// as reclaiming; a status read does not wait for it.
    #[tokio::test]
    async fn running_reclaim_stays_listed_until_terminal() {
        let temp = tempfile::tempdir().unwrap();
        let native = Arc::new(FakeNative::default());
        let worker = worker(Db::open(temp.path()).await.unwrap(), native.clone());
        let (request_id, scope) = ecash_request("ecash-reclaiming");
        let job = worker
            .start_job(&request_id, scope, PayoutRail::Ecash)
            .await
            .unwrap();
        let id = job.operation.unwrap().operation_id;
        {
            let mut notes = native.notes.lock().await;
            let notes = notes.get_mut(&id).unwrap();
            notes.reclaimed = true;
            notes.reclaim_running = true;
        }
        let open = open_payouts(&worker).await;
        assert_eq!(open.len(), 1);
        assert_eq!(ecash_state(&open[0]), &EcashPayoutState::Reclaiming);
        assert_eq!(
            ecash_state(&worker.status(&request_id, true).await.unwrap()),
            &EcashPayoutState::Reclaimed
        );
    }

    /// Confirmation is the operator's word: one ledger write, set once. It
    /// reads no wallet and starts no reclaim, and the notes stay issued.
    #[tokio::test]
    async fn ecash_confirmation_is_a_flag_set_once() {
        let temp = tempfile::tempdir().unwrap();
        let native = Arc::new(FakeNative::default());
        let worker = worker(Db::open(temp.path()).await.unwrap(), native.clone());
        let (request_id, scope) = ecash_request("ecash-confirm");
        worker
            .start_job(&request_id, scope, PayoutRail::Ecash)
            .await
            .unwrap();
        let observed = native.statuses.load(Ordering::SeqCst);

        let confirmed = worker.confirm_ecash_payout(&request_id).await.unwrap();
        let at = confirmed
            .operation
            .as_ref()
            .and_then(|op| op.confirmed_at_ms)
            .expect("the confirmation is on the job");
        assert_eq!(
            worker.confirm_ecash_payout(&request_id).await.unwrap(),
            confirmed,
            "repeating keeps the first confirmation"
        );
        assert_eq!(native.statuses.load(Ordering::SeqCst), observed);
        assert_eq!(native.reclaims.load(Ordering::SeqCst), 0);

        let status = worker.status(&request_id, false).await.unwrap();
        assert!(matches!(
            ecash_state(&status),
            EcashPayoutState::Issued { .. }
        ));
        assert_eq!(confirmed_at(&status.job), Some(at));
        assert!(open_payouts(&worker).await.is_empty());
    }

    /// A token marked redeemed that never arrived can still be taken back:
    /// the reclaim's outcome replaces the confirmation.
    #[tokio::test]
    async fn cancel_after_confirmation_returns_unredeemed_notes() {
        let temp = tempfile::tempdir().unwrap();
        let native = Arc::new(FakeNative::default());
        let worker = worker(Db::open(temp.path()).await.unwrap(), native.clone());
        let (request_id, scope) = ecash_request("ecash-confirm-then-cancel");
        worker
            .start_job(&request_id, scope, PayoutRail::Ecash)
            .await
            .unwrap();
        worker.confirm_ecash_payout(&request_id).await.unwrap();

        let cancelled = worker.cancel(&request_id).await.unwrap();
        assert_eq!(ecash_state(&cancelled), &EcashPayoutState::Reclaimed);
        assert_eq!(native.reclaims.load(Ordering::SeqCst), 1);
        // The mark stays in the ledger beside the wallet's answer.
        assert!(confirmed_at(&cancelled.job).is_some());
    }

    /// When the operator did redeem a token they marked, a later cancel loses
    /// the race and says so; nothing returns.
    #[tokio::test]
    async fn cancel_after_confirmation_of_a_redeemed_token_reports_already_spent() {
        let temp = tempfile::tempdir().unwrap();
        let native = Arc::new(FakeNative::default());
        let worker = worker(Db::open(temp.path()).await.unwrap(), native.clone());
        let (request_id, scope) = ecash_request("ecash-confirm-redeemed-cancel");
        let job = worker
            .start_job(&request_id, scope, PayoutRail::Ecash)
            .await
            .unwrap();
        worker.confirm_ecash_payout(&request_id).await.unwrap();
        native
            .notes
            .lock()
            .await
            .get_mut(&job.operation.unwrap().operation_id)
            .unwrap()
            .redeemed = true;

        assert!(matches!(
            ecash_state(&worker.cancel(&request_id).await.unwrap()),
            EcashPayoutState::AlreadySpent { .. }
        ));
        assert!(matches!(
            ecash_state(&worker.status(&request_id, false).await.unwrap()),
            EcashPayoutState::AlreadySpent { .. }
        ));
    }

    /// A job whose wallet cannot be read is left out of the list, and the
    /// others, an older open token among them, are still listed.
    #[tokio::test]
    async fn list_leaves_out_an_unreadable_job_and_keeps_the_others() {
        let temp = tempfile::tempdir().unwrap();
        let db = Db::open(temp.path()).await.unwrap();
        set_destination(&db, "operator@example.com").await;
        let native = Arc::new(FakeNative::default());
        let worker = worker(db.clone(), native.clone());
        let mut started = Vec::new();
        for (name, rail) in [
            ("a-open-token", PayoutRail::Ecash),
            ("b-unreadable", PayoutRail::Lightning),
            ("c-lightning", PayoutRail::Lightning),
        ] {
            let (request_id, scope) = ecash_request(name);
            worker.start_job(&request_id, scope, rail).await.unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(3)).await;
            started.push(request_id);
        }
        let ids = |listed: &[PayoutJobStatus]| {
            listed
                .iter()
                .map(|s| s.job.request_id.as_str().to_owned())
                .collect::<Vec<_>>()
        };
        *native.unreadable.lock().await = Some(started[1].clone());
        assert_eq!(
            ids(&worker.list().await.unwrap()),
            ["c-lightning", "a-open-token"]
        );
        // An unreadable open token past the recent window is left out too,
        // and comes back once its wallet can be read.
        for n in 0..RECENT_PAYOUTS {
            let (request_id, scope) = ecash_request(&format!("d-newer-{n:02}"));
            worker
                .start_job(&request_id, scope, PayoutRail::Lightning)
                .await
                .unwrap();
        }
        *native.unreadable.lock().await = Some(started[0].clone());
        let listed = ids(&worker.list().await.unwrap());
        assert_eq!(listed.len(), RECENT_PAYOUTS as usize);
        assert!(!listed.contains(&"a-open-token".to_owned()), "{listed:?}");
        *native.unreadable.lock().await = None;
        assert_eq!(
            ids(&worker.list().await.unwrap()).last().unwrap(),
            "a-open-token"
        );
    }

    /// The list holds both methods and closed jobs, newest first, keeps an
    /// older open token past the limit, and never carries a token.
    #[tokio::test]
    async fn list_holds_closed_jobs_of_both_methods_and_keeps_old_open_tokens() {
        let temp = tempfile::tempdir().unwrap();
        let db = Db::open(temp.path()).await.unwrap();
        set_destination(&db, "operator@example.com").await;
        let native = Arc::new(FakeNative::default());
        let worker = worker(db.clone(), native.clone());
        let start = |name: &'static str, rail| {
            let worker = &worker;
            async move {
                let (request_id, scope) = ecash_request(name);
                worker.start_job(&request_id, scope, rail).await.unwrap();
                // Distinct creation times order the list.
                tokio::time::sleep(std::time::Duration::from_millis(3)).await;
                request_id
            }
        };
        let old_open = start("a-old-open", PayoutRail::Ecash).await;
        let reclaimed = start("b-reclaimed", PayoutRail::Ecash).await;
        let confirmed = start("c-confirmed", PayoutRail::Ecash).await;
        let lightning = start("d-lightning", PayoutRail::Lightning).await;
        worker.cancel(&reclaimed).await.unwrap();
        worker.confirm_ecash_payout(&confirmed).await.unwrap();

        let ids = |listed: &[PayoutJobStatus]| {
            listed
                .iter()
                .map(|s| s.job.request_id.as_str().to_owned())
                .collect::<Vec<_>>()
        };
        let all = worker.list().await.unwrap();
        assert_eq!(
            ids(&all),
            ["d-lightning", "c-confirmed", "b-reclaimed", "a-old-open"]
        );
        assert!(matches!(
            all[0].native,
            PayoutNativeStatus::Lightning(Some(_))
        ));
        assert!(matches!(
            ecash_state(&all[1]),
            EcashPayoutState::Issued { .. }
        ));
        assert!(confirmed_at(&all[1].job).is_some());
        assert_eq!(ecash_state(&all[2]), &EcashPayoutState::Reclaimed);

        let wire = serde_json::to_value(worker.list_payouts().await.unwrap()).unwrap();
        assert_eq!(
            wire["payouts"][3]["ecash"],
            serde_json::json!({ "state": "issued", "token": null })
        );
        assert!(!wire.to_string().contains("token-for-"), "{wire}");

        // Twenty newer payouts push the closed ones out; the older issued
        // token still comes along, and the marked one does not.
        let mut newer = Vec::new();
        for n in 0..20 {
            let name = format!("e-newer-{n:02}");
            let (request_id, scope) = ecash_request(&name);
            worker
                .start_job(&request_id, scope, PayoutRail::Lightning)
                .await
                .unwrap();
            newer.insert(0, name);
        }
        newer.push("a-old-open".to_owned());
        assert_eq!(ids(&worker.list().await.unwrap()), newer);
        // The single-job answer still hands the token over.
        let status =
            serde_json::to_value(worker.status(&old_open, false).await.unwrap().to_wire(true))
                .unwrap();
        assert_eq!(status["ecash"]["state"], "issued");
        assert!(status["ecash"]["token"].is_string());
        drop(lightning);
    }

    /// A start the wallet refused (no gateway, an LNURL error) leaves a ledger
    /// row with no operation. It is not a payout: the list leaves it out, so
    /// nothing shows it as in progress forever.
    #[tokio::test]
    async fn a_start_that_never_happened_is_not_listed() {
        let temp = tempfile::tempdir().unwrap();
        let db = Db::open(temp.path()).await.unwrap();
        set_destination(&db, "operator@example.com").await;
        let native = Arc::new(FakeNative::default());
        let worker = worker(db.clone(), native.clone());
        for (name, rail) in [
            ("refused-lightning", PayoutRail::Lightning),
            ("refused-ecash", PayoutRail::Ecash),
        ] {
            let (request_id, scope) = ecash_request(name);
            native.refuse_next_start.store(true, Ordering::SeqCst);
            assert!(worker.start_job(&request_id, scope, rail).await.is_err());
            assert!(
                crate::payout_store::get(&db, &request_id)
                    .await
                    .unwrap()
                    .is_some_and(|job| job.operation.is_none())
            );
        }
        let (started, scope) = ecash_request("started-ecash");
        worker
            .start_job(&started, scope, PayoutRail::Ecash)
            .await
            .unwrap();

        let listed = worker.list().await.unwrap();
        assert_eq!(
            listed
                .iter()
                .map(|s| s.job.request_id.as_str())
                .collect::<Vec<_>>(),
            ["started-ecash"]
        );
    }

    /// The ecash verbs refuse a Lightning job instead of treating it as one.
    #[tokio::test]
    async fn ecash_verbs_refuse_a_lightning_job() {
        let temp = tempfile::tempdir().unwrap();
        let db = Db::open(temp.path()).await.unwrap();
        set_destination(&db, "operator@example.com").await;
        let native = Arc::new(FakeNative::default());
        let worker = worker(db, native.clone());
        let (request_id, scope) = ecash_request("lightning-job");
        worker
            .start_job(&request_id, scope, PayoutRail::Lightning)
            .await
            .unwrap();
        let error = worker.confirm_ecash_payout(&request_id).await.unwrap_err();
        assert!(
            error.to_string().contains("only an ecash payout"),
            "{error}"
        );
        let error = worker.cancel(&request_id).await.unwrap_err();
        assert!(error.to_string().contains("not an ecash payout"), "{error}");
        assert!(open_payouts(&worker).await.is_empty());
        assert_eq!(native.reclaims.load(Ordering::SeqCst), 0);
    }
}

impl PayoutWorker {
    async fn start_job(
        &self,
        request_id: &PayoutRequestId,
        scope: PayoutScope,
        rail: PayoutRail,
    ) -> anyhow::Result<PayoutJob> {
        let job = match crate::payout_store::get(&self.db, request_id).await? {
            Some(j) if j.scope != scope => {
                anyhow::bail!("payout request id is already bound to a different scope")
            }
            Some(j) if j.method.rail() != rail => {
                anyhow::bail!("payout request id is already bound to a different payout method")
            }
            Some(j) => j,
            None => {
                let method = match rail {
                    PayoutRail::Lightning => PayoutMethod::Lightning {
                        destination: crate::payout_store::destination(&self.db)
                            .await?
                            .context("no payout destination configured")?,
                    },
                    PayoutRail::Ecash => PayoutMethod::Ecash,
                };
                crate::payout_store::create(&self.db, request_id, &scope, &method).await?
            }
        };
        validate_scope(&job.scope)?;
        if job.operation.is_some() {
            return Ok(job);
        }
        let payout = self.native.start(&job).await?;
        pause_after_payout_start_for_e2e().await?;
        crate::payout_store::commit(&self.db, request_id, &payout).await
    }

    async fn reconcile(&self, job: PayoutJob, wait: bool) -> anyhow::Result<PayoutJob> {
        validate_scope(&job.scope)?;
        if job.operation.is_some() {
            return Ok(job);
        }
        let payout = self.native.find(&job, wait).await?;
        match payout {
            Some(p) => crate::payout_store::commit(&self.db, &job.request_id, &p).await,
            None => Ok(job),
        }
    }
    async fn status(
        &self,
        request_id: &PayoutRequestId,
        wait: bool,
    ) -> anyhow::Result<PayoutJobStatus> {
        let job = crate::payout_store::get(&self.db, request_id)
            .await?
            .context("unknown payout request id")?;
        self.project(job, wait).await
    }

    /// One job with its native state, read from the wallet.
    async fn project(&self, job: PayoutJob, wait: bool) -> anyhow::Result<PayoutJobStatus> {
        let job = self.reconcile(job, wait).await?;
        let native = match (&job.method, &job.operation) {
            (_, None) if wait => {
                anyhow::bail!("payout request has no committed native operation")
            }
            (PayoutMethod::Lightning { .. }, None) => PayoutNativeStatus::Lightning(None),
            (PayoutMethod::Ecash, None) => PayoutNativeStatus::Ecash(None),
            (_, Some(op)) => self.observe(&job, op.operation_id.clone(), wait).await?,
        };
        Ok(PayoutJobStatus { job, native })
    }
    async fn observe(
        &self,
        job: &PayoutJob,
        id: PayoutOperationId,
        wait: bool,
    ) -> anyhow::Result<PayoutNativeStatus> {
        validate_scope(&job.scope)?;
        self.native.observe(job, id, wait).await
    }

    /// The committed, linked ecash job for a reclaim.
    async fn issued_ecash_job(
        &self,
        request_id: &PayoutRequestId,
    ) -> anyhow::Result<(PayoutJob, PayoutOperationId)> {
        let job = crate::payout_store::get(&self.db, request_id)
            .await?
            .context("unknown payout request id")?;
        anyhow::ensure!(
            job.method == PayoutMethod::Ecash,
            "payout request is not an ecash payout"
        );
        let job = self.reconcile(job, true).await?;
        let id = job
            .operation
            .as_ref()
            .map(|op| op.operation_id.clone())
            .context("ecash payout has no issued token")?;
        Ok((job, id))
    }

    /// Submit the reclaim of a payout's notes, then await the federation's
    /// answer. A confirmation does not prevent it: the operator's word that a
    /// token arrived can be wrong, and the reclaim itself finds out whether
    /// the notes were spent.
    async fn cancel(&self, request_id: &PayoutRequestId) -> anyhow::Result<PayoutJobStatus> {
        let (job, id) = self.issued_ecash_job(request_id).await?;
        validate_scope(&job.scope)?;
        self.native.reclaim(&job, id).await?;
        self.status(request_id, true).await
    }

    /// One listed job, or `None` when its wallet cannot be read now. One
    /// unreadable federation must not hide every other payout.
    async fn project_listed(&self, job: PayoutJob) -> Option<PayoutJobStatus> {
        let request_id = job.request_id.clone();
        self.project(job, false)
            .await
            .inspect_err(|error| {
                tracing::warn!(%request_id, error = %format!("{error:#}"), "payout left out of the list: its wallet is unreadable");
            })
            .ok()
    }

    /// The newest [`RECENT_PAYOUTS`] jobs of both methods, plus every older
    /// ecash job still in progress, newest first. An issued token or a running reclaim
    /// is never pushed out of the list by newer payouts. A job left without a
    /// link by a crash is reconciled first, so a committed token is never
    /// hidden. A job that still has no operation after that is a start that
    /// never happened, not a payout, and is left out, as is a job whose wallet
    /// cannot be read now.
    async fn list(&self) -> anyhow::Result<Vec<PayoutJobStatus>> {
        let recent = crate::payout_store::recent(&self.db, RECENT_PAYOUTS).await?;
        let mut listed = Vec::with_capacity(recent.len());
        for job in crate::payout_store::unconfirmed_ecash(&self.db).await? {
            if recent.iter().any(|r| r.request_id == job.request_id) {
                continue;
            }
            let Some(status) = self.project_listed(job).await else {
                continue;
            };
            if matches!(&status.native, PayoutNativeStatus::Ecash(Some(state)) if state.is_open()) {
                listed.push(status);
            }
        }
        for job in recent {
            let Some(status) = self.project_listed(job).await else {
                continue;
            };
            if status.job.operation.is_some() {
                listed.push(status);
            }
        }
        listed.sort_by(|a, b| {
            (b.job.created_at_ms, &b.job.request_id).cmp(&(a.job.created_at_ms, &a.job.request_id))
        });
        Ok(listed)
    }
}

/// Reject a restored guardian row whose redundant public federation identity
/// does not agree with its retained invite before it can select a wallet.
fn validate_scope(scope: &PayoutScope) -> anyhow::Result<()> {
    if let PayoutScope::GuardianFee {
        federation_id,
        invite_code,
        ..
    } = scope
    {
        anyhow::ensure!(
            federation_id.0 == invite_code.federation_id().to_string(),
            "payout request wallet scope no longer matches the seat federation"
        );
    }
    Ok(())
}

impl WalletPayoutNative {
    /// The wallet's fence identity for one payout scope.
    fn client_scope(scope: &PayoutScope) -> anyhow::Result<ClientScope> {
        Ok(match scope {
            PayoutScope::PaymentFederation { federation_id } => ClientScope::Payment(
                federation_id
                    .0
                    .parse()
                    .map_err(|_| anyhow::anyhow!("malformed federation id"))?,
            ),
            PayoutScope::GuardianFee {
                invite_code,
                seat_id,
                ..
            } => ClientScope::Guardian {
                federation_id: invite_code.federation_id(),
                seat_id: seat_id.to_string(),
            },
        })
    }

    /// Open the ordinary client of one payout scope. A caller that does not
    /// `Wait` is refused while another task opens the wallet, and never joins a
    /// guardian-fee wallet that was not joined yet: a join downloads the
    /// federation's config, which can take as long as its timeout.
    async fn client(&self, scope: &PayoutScope, wait: OpenWait) -> anyhow::Result<ClientHandleArc> {
        Ok(match scope {
            PayoutScope::PaymentFederation { federation_id } => {
                self.wallet
                    .client_with(federation_id.0.parse()?, wait)
                    .await?
            }
            PayoutScope::GuardianFee {
                invite_code,
                seat_id,
                ..
            } => {
                anyhow::ensure!(
                    wait == OpenWait::Wait
                        || self
                            .wallet
                            .scope_initialized(&Self::client_scope(scope)?)
                            .await,
                    "guardian-fee wallet is not joined yet"
                );
                let key = (self.guardian_key)(seat_id);
                self.wallet
                    .guardian_fee_client(invite_code, seat_id, &key, wait)
                    .await?
            }
        })
    }
}

#[async_trait::async_trait]
impl PayoutNative for WalletPayoutNative {
    async fn start(&self, job: &PayoutJob) -> anyhow::Result<Payout> {
        let _lock = self
            .wallet
            .payout_exclusion(Self::client_scope(&job.scope)?)
            .await;
        let client = self.client(&job.scope, OpenWait::Wait).await?;
        let PayoutMethod::Lightning { destination } = &job.method else {
            return crate::payout_ecash::start_ecash_payout(&client, &job.request_id).await;
        };
        // Under the fence, the ledger's attempt is the last one any caller
        // wrote, and no other caller is paying it.
        let job = crate::payout_store::get(&self.db, &job.request_id)
            .await?
            .context("unknown payout request id")?;
        if let Some(operation) = &job.operation {
            return Ok(Payout {
                operation_id: operation.operation_id.clone(),
                amount_msat: operation.amount_msat,
            });
        }
        if let Some(payout) = crate::payout_native::attempted_payout(&client, &job).await {
            return Ok(payout);
        }
        // No operation exists for the recorded attempt, if any, so nothing
        // was paid and a fresh attempt replaces it.
        let prepared = crate::payout_native::prepare_payout(&client, destination).await?;
        crate::payout_store::attempt(&self.db, &job.request_id, &prepared.attempt).await?;
        crate::payout_native::pay_payout(&client, &job.request_id, destination, prepared).await
    }

    async fn find(&self, job: &PayoutJob, wait: bool) -> anyhow::Result<Option<Payout>> {
        let client = self.client(&job.scope, OpenWait::when(wait)).await?;
        Ok(match &job.method {
            PayoutMethod::Lightning { .. } => {
                crate::payout_native::attempted_payout(&client, job).await
            }
            PayoutMethod::Ecash => {
                crate::payout_ecash::find_ecash_payout(
                    &client,
                    &PayoutOperationId::ecash(&job.request_id),
                )
                .await?
            }
        })
    }

    async fn observe(
        &self,
        job: &PayoutJob,
        id: PayoutOperationId,
        wait: bool,
    ) -> anyhow::Result<PayoutNativeStatus> {
        let client = self.client(&job.scope, OpenWait::when(wait)).await?;
        Ok(match &job.method {
            PayoutMethod::Lightning { .. } => PayoutNativeStatus::Lightning(Some(if wait {
                crate::await_payout(&client, &id).await?
            } else {
                settled_status(&client, &id).await?
            })),
            PayoutMethod::Ecash => PayoutNativeStatus::Ecash(Some(
                crate::payout_ecash::ecash_payout_state(&client, &id, wait).await?,
            )),
        })
    }

    async fn reclaim(&self, job: &PayoutJob, id: PayoutOperationId) -> anyhow::Result<()> {
        anyhow::ensure!(
            job.method == PayoutMethod::Ecash,
            "only an ecash payout can be reclaimed"
        );
        let _lock = self
            .wallet
            .payout_exclusion(Self::client_scope(&job.scope)?)
            .await;
        let client = self.client(&job.scope, OpenWait::Wait).await?;
        crate::payout_ecash::start_ecash_reclaim(&client, &job.request_id, &id).await
    }
}

/// How long a status read may take to replay a finished payment's updates.
const SETTLED_REPLAY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// The Lightning payout's state without waiting for it to finish. Fedimint
/// caches an operation's outcome only when a subscriber drains its updates,
/// so a payment nobody awaited reads `unknown` once its state machines stop.
/// Its updates then replay from the database at once, so draining them here
/// caches and reports the real outcome.
async fn settled_status(
    client: &ClientHandleArc,
    id: &PayoutOperationId,
) -> anyhow::Result<crate::wallet_drain::OutgoingOperation> {
    let status = crate::payout_status(client, id).await?;
    if status.state() != crate::wallet_drain::OutgoingState::Unknown
        || status.has_active_state_machines()
    {
        return Ok(status);
    }
    match tokio::time::timeout(SETTLED_REPLAY_TIMEOUT, crate::await_payout(client, id)).await {
        Ok(Ok(settled)) => Ok(settled),
        Ok(Err(error)) => {
            tracing::warn!(error = %format!("{error:#}"), "could not replay a finished payout's outcome");
            Ok(status)
        }
        Err(_) => Ok(status),
    }
}

async fn pause_after_payout_start_for_e2e() -> anyhow::Result<()> {
    if std::env::var_os("MG_E2E_LOCAL_IROH").is_none() {
        return Ok(());
    }
    let Some(path) = std::env::var_os("MG_E2E_PAUSE_AFTER_PAYOUT_START") else {
        return Ok(());
    };
    tokio::fs::write(&path, b"native payout committed\n").await?;
    std::future::pending::<()>().await;
    unreachable!()
}
