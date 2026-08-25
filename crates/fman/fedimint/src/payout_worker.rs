//! Durable caller-idempotent payout orchestration owned beside the native wallet.

use anyhow::Context as _;
use fedi_decentralized_service_fleet_manager::{
    FederationId, InviteCode as WireInviteCode, SeatId,
};
use fedimint_core::invite_code::InviteCode;
use fman_core::db::Db;
use fman_core::guardian_fee::GuardianFeeAccountKey;
use fman_core::payout_wire::{PayoutJobStatusWire, PayoutJobWire, WalletDrainStatusWire};
use fman_core::wallet::{EcashPayoutWorker, PayoutRequestId};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::payout_job::{Payout, PayoutJob, PayoutScope};
use crate::payout_job_status::PayoutJobStatus;
use crate::wallet_drain::{OutgoingOperation, WalletDrainStatus};
use crate::{ClientScope, Wallet};

pub(crate) struct PayoutWorker {
    wallet: Option<Arc<Wallet>>,
    db: Db,
    native: Arc<dyn PayoutNative>,
    guardian_key: Arc<dyn Fn(&SeatId) -> GuardianFeeAccountKey + Send + Sync>,
    start_locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
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
                guardian_key: guardian_key.clone(),
            }),
            wallet: Some(wallet),
            db,
            guardian_key,
            start_locks: Mutex::new(HashMap::new()),
        })
    }
}

/// Wallet-side effects required by the durable payout orchestrator.
///
/// The worker owns an outer per-scope replay fence; the production adapter owns
/// Fedimint client opening and its inner native-operation fence. Keeping this
/// seam here makes the worker's SQLite/replay fence independently testable
/// without treating a test double as the core boundary.
#[async_trait::async_trait]
trait PayoutNative: Send + Sync {
    /// Start a native payout or recover its prior native commit by request id.
    async fn start_or_recover(&self, job: &PayoutJob) -> anyhow::Result<Payout>;

    /// Find a native payout by its request id without starting one.
    async fn find(&self, job: &PayoutJob) -> anyhow::Result<Option<Payout>>;

    /// Read or await one already committed native payout.
    async fn observe(
        &self,
        job: &PayoutJob,
        id: crate::payout_operation_id::PayoutOperationId,
        wait: bool,
    ) -> anyhow::Result<OutgoingOperation>;
}

/// Production Fedimint implementation of [`PayoutNative`].
struct WalletPayoutNative {
    /// Shared wallet that opens the client for one persisted payout scope.
    wallet: Arc<Wallet>,
    /// Mnemonic-derived key lookup for a guardian-fee wallet scope.
    guardian_key: Arc<dyn Fn(&SeatId) -> GuardianFeeAccountKey + Send + Sync>,
}

#[async_trait::async_trait]
impl EcashPayoutWorker for PayoutWorker {
    async fn sweep_payment_fees(
        &self,
        federation_id: &FederationId,
        request_id: &PayoutRequestId,
    ) -> anyhow::Result<PayoutJobWire> {
        let scope = PayoutScope::PaymentFederation {
            federation_id: federation_id.clone(),
        };
        let job = self.start_job(request_id, scope).await?;
        Ok(job.to_wire())
    }
    async fn sweep_guardian_fees(
        &self,
        invite: &WireInviteCode,
        seat_id: &SeatId,
        request_id: &PayoutRequestId,
    ) -> anyhow::Result<PayoutJobWire> {
        let invite: InviteCode = invite
            .0
            .parse()
            .context("invalid guardian federation invite")?;
        let scope = PayoutScope::GuardianFee {
            federation_id: FederationId(invite.federation_id().to_string()),
            seat_id: seat_id.clone(),
            invite_code: invite.clone(),
        };
        let job = self.start_job(request_id, scope).await?;
        Ok(job.to_wire())
    }
    async fn resume_guardian_sweep(
        &self,
        seat_id: &SeatId,
        request_id: &PayoutRequestId,
    ) -> anyhow::Result<Option<PayoutJobWire>> {
        let Some(job) = stored_guardian_job(&self.db, seat_id, request_id).await? else {
            return Ok(None);
        };
        let job = self.start_job(request_id, job.scope.clone()).await?;
        Ok(Some(job.to_wire()))
    }
    async fn payout_status(
        &self,
        request_id: &PayoutRequestId,
    ) -> anyhow::Result<PayoutJobStatusWire> {
        Ok(self.status(request_id, false).await?.to_wire())
    }
    async fn await_payout(
        &self,
        request_id: &PayoutRequestId,
    ) -> anyhow::Result<PayoutJobStatusWire> {
        Ok(self.status(request_id, true).await?.to_wire())
    }
    async fn payment_drain_status(&self, federation_id: &FederationId) -> WalletDrainStatusWire {
        let status = match (self.wallet.as_ref(), federation_id.0.parse()) {
            (Some(wallet), Ok(id)) => match wallet.client(id).await {
                Ok(c) => crate::drain_status::wallet_drain_status(&c).await,
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
        let invite: InviteCode = invite
            .0
            .parse()
            .context("invalid guardian federation invite")?;
        let Some(wallet) = &self.wallet else {
            return Ok(WalletDrainStatus::unavailable().to_wire());
        };
        let scope = PayoutScope::GuardianFee {
            federation_id: FederationId(invite.federation_id().to_string()),
            seat_id: seat_id.clone(),
            invite_code: invite.clone(),
        };
        validate_scope(&scope)?;
        let key = (self.guardian_key)(seat_id);
        match wallet.guardian_fee_client(&invite, seat_id, &key).await {
            Ok(c) => Ok(crate::drain_status::wallet_drain_status(&c).await.to_wire()),
            Err(_) => Ok(WalletDrainStatus::unavailable().to_wire()),
        }
    }
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
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    #[derive(Default)]
    struct FakeNative {
        payouts: Mutex<HashMap<PayoutRequestId, Payout>>,
        lose_next_response: AtomicBool,
        starts: AtomicUsize,
        statuses: AtomicUsize,
        awaits: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl PayoutNative for FakeNative {
        async fn start_or_recover(&self, job: &PayoutJob) -> anyhow::Result<Payout> {
            if let Some(payout) = self.payouts.lock().await.get(&job.request_id).cloned() {
                return Ok(payout);
            }
            self.starts.fetch_add(1, Ordering::SeqCst);
            // A second caller reaches this yield before the first writes if
            // PayoutWorker stops holding its scope exclusion across the
            // native lookup/start boundary.
            tokio::task::yield_now().await;
            let payout = Payout {
                operation_id: crate::payout_operation_id::PayoutOperationId::parse(
                    &"ab".repeat(32),
                )?,
                amount_msat: 42,
            };
            self.payouts
                .lock()
                .await
                .insert(job.request_id.clone(), payout.clone());
            if self.lose_next_response.swap(false, Ordering::SeqCst) {
                anyhow::bail!("simulated native response loss");
            }
            Ok(payout)
        }

        async fn find(&self, job: &PayoutJob) -> anyhow::Result<Option<Payout>> {
            Ok(self.payouts.lock().await.get(&job.request_id).cloned())
        }

        async fn observe(
            &self,
            _job: &PayoutJob,
            id: crate::payout_operation_id::PayoutOperationId,
            wait: bool,
        ) -> anyhow::Result<OutgoingOperation> {
            if wait {
                self.awaits.fetch_add(1, Ordering::SeqCst);
            } else {
                self.statuses.fetch_add(1, Ordering::SeqCst);
            }
            Ok(OutgoingOperation::new(
                id,
                crate::wallet_drain::OutgoingRail::Lnv2,
                crate::wallet_drain::OutgoingState::Succeeded,
                42,
                42,
                false,
            ))
        }
    }

    fn worker(db: Db, native: Arc<FakeNative>) -> PayoutWorker {
        PayoutWorker {
            wallet: None,
            db,
            native,
            guardian_key: Arc::new(|_| GuardianFeeAccountKey::from_secret_bytes(&[1; 32])),
            start_locks: Mutex::new(HashMap::new()),
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
        crate::payout_store::create(&db, &request_id, &scope, "operator@example.com")
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
        crate::payout_store::create(&db, &mismatch, &malformed, "operator@example.com")
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
                .start_job(&request_id, scope.clone())
                .await
                .unwrap_err()
                .to_string()
                .contains("response loss")
        );
        drop(db);

        let reopened = Db::open(temp.path()).await.unwrap();
        set_destination(&reopened, "changed@example.com").await;
        let job = worker(reopened, native.clone())
            .start_job(&request_id, scope)
            .await
            .unwrap();
        assert_eq!(native.starts.load(Ordering::SeqCst), 1);
        assert_eq!(job.destination, "first@example.com");
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
            worker.start_job(&request_id, scope.clone()),
            worker.start_job(&request_id, scope),
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
        crate::payout_store::create(&db, &request_id, &scope, "operator@example.com")
            .await
            .unwrap();
        let native = Arc::new(FakeNative::default());
        let status = worker(db, native.clone())
            .status(&request_id, false)
            .await
            .unwrap();
        assert!(status.job.operation.is_none());
        assert!(status.payout.is_none());
        assert_eq!(native.starts.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn guardian_status_and_await_replay_without_a_live_seat() {
        let temp = tempfile::tempdir().unwrap();
        let db = Db::open(temp.path()).await.unwrap();
        let seat_id = SeatId::new("71".repeat(32)).unwrap();
        let request_id = PayoutRequestId::parse("guardian-lost-response").unwrap();
        let scope = guardian_scope(&seat_id, &"02".repeat(32));
        crate::payout_store::create(&db, &request_id, &scope, "operator@example.com")
            .await
            .unwrap();
        let native = Arc::new(FakeNative {
            lose_next_response: AtomicBool::new(true),
            ..Default::default()
        });
        let current = worker(db.clone(), native.clone());
        assert!(
            current
                .start_job(&request_id, scope)
                .await
                .unwrap_err()
                .to_string()
                .contains("response loss")
        );
        drop(current);
        drop(db);

        let reopened = worker(Db::open(temp.path()).await.unwrap(), native.clone());
        let status = reopened.status(&request_id, false).await.unwrap();
        assert!(status.payout.is_some());
        let awaited = reopened.status(&request_id, true).await.unwrap();
        assert!(awaited.payout.is_some());
        assert_eq!(native.starts.load(Ordering::SeqCst), 1);
        assert_eq!(native.statuses.load(Ordering::SeqCst), 1);
        assert_eq!(native.awaits.load(Ordering::SeqCst), 1);
    }
}
impl PayoutWorker {
    async fn start_job(
        &self,
        request_id: &PayoutRequestId,
        scope: PayoutScope,
    ) -> anyhow::Result<PayoutJob> {
        let job = match crate::payout_store::get(&self.db, request_id).await? {
            Some(j) if j.scope == scope => j,
            Some(_) => anyhow::bail!("payout request id is already bound to a different scope"),
            None => {
                let dest = crate::payout_store::destination(&self.db)
                    .await?
                    .context("no payout destination configured")?;
                crate::payout_store::create(&self.db, request_id, &scope, &dest).await?
            }
        };
        validate_scope(&job.scope)?;
        if job.operation.is_some() {
            return Ok(job);
        }
        let _start = self.start_exclusion(&job.scope).await;
        let payout = self.start_native(&job).await?;
        if matches!(job.scope, PayoutScope::GuardianFee { .. }) {
            pause_after_guardian_fee_payout_start_for_e2e().await?;
        }
        crate::payout_store::commit(&self.db, request_id, &payout).await
    }

    /// Serialize start-or-recover attempts for one immutable wallet scope.
    ///
    /// The native wallet repeats this exclusion around its durable operation
    /// lookup and start. Keeping the worker fence at the ownership boundary
    /// makes request replay safe for every native implementation and makes the
    /// same invariant directly testable here.
    async fn start_exclusion(&self, scope: &PayoutScope) -> tokio::sync::OwnedMutexGuard<()> {
        let key = match scope {
            PayoutScope::PaymentFederation { federation_id } => {
                format!("payment:{}", federation_id.0)
            }
            PayoutScope::GuardianFee {
                federation_id,
                seat_id,
                ..
            } => format!("guardian:{}:{seat_id}", federation_id.0),
        };
        let lock = self
            .start_locks
            .lock()
            .await
            .entry(key)
            .or_default()
            .clone();
        lock.lock_owned().await
    }
    async fn start_native(&self, job: &PayoutJob) -> anyhow::Result<Payout> {
        validate_scope(&job.scope)?;
        self.native.start_or_recover(job).await
    }
    async fn reconcile(&self, job: PayoutJob) -> anyhow::Result<PayoutJob> {
        validate_scope(&job.scope)?;
        if job.operation.is_some() {
            return Ok(job);
        }
        let payout = self.native.find(&job).await?;
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
        let job = self.reconcile(job).await?;
        let payout = if let Some(op) = &job.operation {
            Some(self.observe(&job, op.operation_id.clone(), wait).await?)
        } else if wait {
            anyhow::bail!("payout request has no committed native operation")
        } else {
            None
        };
        Ok(PayoutJobStatus { job, payout })
    }
    async fn observe(
        &self,
        job: &PayoutJob,
        id: crate::payout_operation_id::PayoutOperationId,
        wait: bool,
    ) -> anyhow::Result<OutgoingOperation> {
        validate_scope(&job.scope)?;
        self.native.observe(job, id, wait).await
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

#[async_trait::async_trait]
impl PayoutNative for WalletPayoutNative {
    async fn start_or_recover(&self, job: &PayoutJob) -> anyhow::Result<Payout> {
        match &job.scope {
            PayoutScope::PaymentFederation { federation_id } => {
                let id = federation_id
                    .0
                    .parse()
                    .map_err(|_| anyhow::anyhow!("malformed federation id"))?;
                let _lock = self.wallet.payout_exclusion(ClientScope::Payment(id)).await;
                let client = self.wallet.client(id).await?;
                if let Some(payout) =
                    crate::payout_for_request(&client, &job.request_id, &job.destination).await?
                {
                    Ok(payout)
                } else {
                    crate::start_payout(&client, &job.request_id, &job.destination).await
                }
            }
            PayoutScope::GuardianFee {
                invite_code,
                seat_id,
                ..
            } => {
                let key = (self.guardian_key)(seat_id);
                let _lock = self
                    .wallet
                    .payout_exclusion(ClientScope::Guardian {
                        federation_id: invite_code.federation_id(),
                        seat_id: seat_id.to_string(),
                    })
                    .await;
                let client = self
                    .wallet
                    .guardian_fee_client(invite_code, seat_id, &key)
                    .await?;
                if let Some(payout) =
                    crate::payout_for_request(&client, &job.request_id, &job.destination).await?
                {
                    Ok(payout)
                } else {
                    crate::start_payout(&client, &job.request_id, &job.destination).await
                }
            }
        }
    }

    async fn find(&self, job: &PayoutJob) -> anyhow::Result<Option<Payout>> {
        match &job.scope {
            PayoutScope::PaymentFederation { federation_id } => {
                let client = self.wallet.client(federation_id.0.parse()?).await?;
                crate::payout_for_request(&client, &job.request_id, &job.destination).await
            }
            PayoutScope::GuardianFee {
                invite_code,
                seat_id,
                ..
            } => {
                let key = (self.guardian_key)(seat_id);
                let client = self
                    .wallet
                    .guardian_fee_client(invite_code, seat_id, &key)
                    .await?;
                crate::payout_for_request(&client, &job.request_id, &job.destination).await
            }
        }
    }

    async fn observe(
        &self,
        job: &PayoutJob,
        id: crate::payout_operation_id::PayoutOperationId,
        wait: bool,
    ) -> anyhow::Result<OutgoingOperation> {
        let client = match &job.scope {
            PayoutScope::PaymentFederation { federation_id } => {
                self.wallet.client(federation_id.0.parse()?).await?
            }
            PayoutScope::GuardianFee {
                invite_code,
                seat_id,
                ..
            } => {
                let key = (self.guardian_key)(seat_id);
                self.wallet
                    .guardian_fee_client(invite_code, seat_id, &key)
                    .await?
            }
        };
        if wait {
            crate::await_payout(&client, &job.request_id, &job.destination, &id).await
        } else {
            crate::payout_status(&client, &job.request_id, &job.destination, &id).await
        }
    }
}

async fn pause_after_guardian_fee_payout_start_for_e2e() -> anyhow::Result<()> {
    if std::env::var_os("FMAN_E2E_LOCAL_IROH").is_none() {
        return Ok(());
    }
    let Some(path) = std::env::var_os("FMAN_E2E_PAUSE_AFTER_GUARDIAN_FEE_PAYOUT_START") else {
        return Ok(());
    };
    tokio::fs::write(&path, b"native payout committed\n").await?;
    std::future::pending::<()>().await;
    unreachable!()
}
