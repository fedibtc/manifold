//! The daemon's operator-owned setup workflow.
//!
//! A fleet opens only after the operator has created or restored its identity,
//! enrolled a Holder authorization, and configured its initial offer.

mod seat_capacity;

use std::sync::Arc;

use serde_json::{Value, json};

use crate::admin::AdminRequest;
use crate::backup::RecoveredFleet;
use crate::db::{Db, OnboardingStage};
use crate::directory::{DirectoryPresence, OnboardingStatus};
use crate::identity::RootMnemonic;
use crate::restore::RestoreError;
use crate::seat_process::SeatProcessConfig;
use crate::wallet::Msats;
use fedi_decentralized_domain::HolderAuthorizationEnvelope;

pub struct FetchedHolderAuthorization {
    pub credential_digest: Vec<u8>,
    pub authorization_issued_at: u64,
    pub event_json: String,
    pub authorization: HolderAuthorizationEnvelope,
}

#[async_trait::async_trait]
pub trait HolderAuthorizationFetcher: Send + Sync {
    async fn retained(
        &self,
        identity: &RootMnemonic,
    ) -> anyhow::Result<Vec<HolderAuthorizationEnvelope>>;

    async fn fetch(
        &self,
        identity: &RootMnemonic,
    ) -> anyhow::Result<(Vec<FetchedHolderAuthorization>, u64)>;
}

#[derive(Debug, thiserror::Error)]
#[error("this Fleet Manager has not completed onboarding")]
pub struct NotOnboarded;

pub(crate) async fn onboard_as_new(db: &Db) -> anyhow::Result<RootMnemonic> {
    let identity = RootMnemonic::generate()?;
    db.install_identity(&identity).await?;
    tracing::info!(safe_to_share = true, "created a new Fleet Manager identity");
    Ok(identity)
}

pub struct Onboarding {
    /// Held across a whole answer, so each answer sees the stage the previous
    /// one made durable.
    operation: tokio::sync::Mutex<()>,
    /// Raised by the answer that settles the final stage. The database is the
    /// durable truth; this only spares [`Onboarding::completed`] from polling.
    completed: tokio::sync::watch::Sender<bool>,
    db: Db,
    process: SeatProcessConfig,
    archive: Arc<dyn crate::backup::BackupArchive>,
    holder_authorizations: Arc<dyn HolderAuthorizationFetcher>,
    holder_status: tokio::sync::Mutex<OnboardingStatus>,
    recommended_max_seats: u32,
    setup_payments_configured: bool,
}

impl Onboarding {
    pub fn new(
        db: Db,
        process: SeatProcessConfig,
        archive: Arc<dyn crate::backup::BackupArchive>,
        holder_authorizations: Arc<dyn HolderAuthorizationFetcher>,
        setup_payments_configured: bool,
    ) -> Arc<Self> {
        Arc::new(Self {
            operation: tokio::sync::Mutex::new(()),
            completed: tokio::sync::watch::Sender::new(false),
            db,
            process,
            archive,
            holder_authorizations,
            holder_status: tokio::sync::Mutex::new(OnboardingStatus::Checking),
            recommended_max_seats: seat_capacity::detect_available_ram_bytes()
                .map(seat_capacity::recommended_max_seats)
                .unwrap_or(0),
            setup_payments_configured,
        })
    }

    /// Resolve once every onboarding stage is durable and the fleet may open.
    ///
    /// Resolves immediately on a daemon that onboarded in an earlier life: the
    /// watch only mirrors this process, so the stage is read from the database
    /// first — subscribing before reading, so a stage settled between the two
    /// is not missed.
    pub async fn completed(&self) -> anyhow::Result<()> {
        let mut completed = self.completed.subscribe();
        if self.db.onboarding_stage().await? == OnboardingStage::Complete {
            return Ok(());
        }
        while !*completed.borrow_and_update() {
            completed.changed().await.expect("the sender lives in self");
        }
        Ok(())
    }

    pub async fn answer(&self, request: AdminRequest) -> anyhow::Result<Value> {
        let _one_at_a_time = self.operation.lock().await;
        match self.db.onboarding_stage().await? {
            OnboardingStage::Identity => self.answer_identity(request).await,
            OnboardingStage::HolderAuthorization => self.answer_holder_authorization(request).await,
            OnboardingStage::InitialOffer => self.answer_initial_offer(request).await,
            OnboardingStage::Complete => match request {
                AdminRequest::Onboarding => self.status_json().await,
                _ => already_completed(&request),
            },
        }
    }

    async fn answer_identity(&self, request: AdminRequest) -> anyhow::Result<Value> {
        match request {
            AdminRequest::OnboardAsNew { .. } => {
                onboard_as_new(&self.db).await?;
                Ok(onboarded_new_json())
            }
            AdminRequest::OnboardFromBackup {
                mnemonic,
                acknowledge_original_host_is_gone,
            } => {
                let recovered = self
                    .restore(&mnemonic, acknowledge_original_host_is_gone)
                    .await?;
                tracing::info!(
                    safe_to_share = true,
                    seats = recovered.seats.len(),
                    formed = recovered.formed(),
                    "restored a Fleet Manager identity and seats"
                );
                Ok(onboarded_restored_json(
                    recovered.seats.len(),
                    recovered.formed(),
                ))
            }
            // Every other verb needs a fleet, and there is no fleet until the
            // identity exists. Saying so — with the discriminant, not only the
            // sentence — is more useful than a connection that refuses to
            // answer.
            _ => Err(NotOnboarded.into()),
        }
    }

    async fn answer_holder_authorization(&self, request: AdminRequest) -> anyhow::Result<Value> {
        match request {
            AdminRequest::Onboarding => self.status_json().await,
            AdminRequest::RefreshHolderAuthorizations => {
                let identity = self.identity().await?;
                match self.holder_authorizations.fetch(&identity).await {
                    Ok((fetched, max_issued_at)) => {
                        let rows = fetched
                            .iter()
                            .map(|event| {
                                (
                                    event.credential_digest.clone(),
                                    event.authorization_issued_at,
                                    event.event_json.clone(),
                                )
                            })
                            .collect::<Vec<_>>();
                        self.db
                            .merge_holder_authorization_events(&rows, max_issued_at)
                            .await?;
                        *self.holder_status.lock().await = observed_status(
                            &fetched
                                .into_iter()
                                .map(|event| event.authorization)
                                .collect::<Vec<_>>(),
                            Some(now_secs()),
                        );
                    }
                    Err(error) => {
                        *self.holder_status.lock().await = OnboardingStatus::RelayError {
                            error: format!("{error:#}"),
                        };
                    }
                }
                self.status_json().await
            }
            AdminRequest::ShowMnemonic => Ok(crate::admin::mnemonic_json(
                &self.identity().await?.phrase(),
            )),
            AdminRequest::OnboardAsNew { if_needed: true } => {
                Ok(crate::admin::onboarded_already_json())
            }
            AdminRequest::OnboardAsNew { .. } | AdminRequest::OnboardFromBackup { .. } => {
                Err(RestoreError::AlreadyOnboarded.into())
            }
            _ => Err(NotOnboarded.into()),
        }
    }

    async fn answer_initial_offer(&self, request: AdminRequest) -> anyhow::Result<Value> {
        match request {
            AdminRequest::Onboarding => self.status_json().await,
            AdminRequest::ShowMnemonic => Ok(crate::admin::mnemonic_json(
                &self.identity().await?.phrase(),
            )),
            AdminRequest::ConfigureInitialOffer {
                max_seats,
                price_msats,
            } => {
                if price_msats.is_some_and(|price| price > 0) && !self.setup_payments_configured {
                    anyhow::bail!(
                        "this environment has no setup-payment publisher, so a paid seat could never be paid for"
                    );
                }
                self.db
                    .configure_initial_offer(price_msats.map(Msats), max_seats)
                    .await?;
                // Raised only after the final stage is durable.
                self.completed.send_replace(true);
                Ok(json!({
                    "onboarding": "complete",
                    "max_seats": max_seats,
                    "plans": price_msats.map(|price_msats| json!({
                        "InfiniteBestEffort": { "price_msats": price_msats }
                    })).into_iter().collect::<Vec<_>>(),
                }))
            }
            AdminRequest::OnboardAsNew { if_needed: true } => {
                Ok(crate::admin::onboarded_already_json())
            }
            AdminRequest::OnboardAsNew { .. } | AdminRequest::OnboardFromBackup { .. } => {
                Err(RestoreError::AlreadyOnboarded.into())
            }
            _ => Err(NotOnboarded.into()),
        }
    }

    async fn status_json(&self) -> anyhow::Result<Value> {
        let identity = self.identity().await?;
        let retained = self.holder_authorizations.retained(&identity).await?;
        let status = if retained.is_empty() {
            self.holder_status.lock().await.clone()
        } else {
            observed_status(&retained, None)
        };
        let directory = DirectoryPresence {
            service_nostr_pubkey: identity.derive_service_nostr_keys().public_key(),
            onboarding: status,
            latest_fman_version: None,
        };
        let current_version = env!("CARGO_PKG_VERSION").parse().expect("valid version");
        let mut value = crate::admin::onboarding_json(
            &identity.derive_service_pubkey().to_string(),
            &directory,
            &current_version,
        );
        let object = value
            .as_object_mut()
            .expect("onboarding response is an object");
        object.insert(
            "stage".to_owned(),
            json!(match self.db.onboarding_stage().await? {
                OnboardingStage::Identity => "identity",
                OnboardingStage::HolderAuthorization => "holder_authorization",
                OnboardingStage::InitialOffer => "initial_offer",
                OnboardingStage::Complete => "complete",
            }),
        );
        object.insert(
            "recommended_max_seats".to_owned(),
            json!(self.recommended_max_seats),
        );
        object.insert(
            "minimum_max_seats".to_owned(),
            json!(self.db.active_seat_count().await?),
        );
        object.insert("runtime".to_owned(), json!("starting"));
        Ok(value)
    }

    async fn identity(&self) -> anyhow::Result<RootMnemonic> {
        self.db
            .load_identity()
            .await?
            .ok_or_else(|| anyhow::anyhow!("post-identity onboarding stage has no identity"))
    }

    async fn restore(
        &self,
        mnemonic: &str,
        acknowledged: bool,
    ) -> Result<RecoveredFleet, RestoreError> {
        if !acknowledged {
            return Err(RestoreError::NotAcknowledged);
        }
        let identity =
            RootMnemonic::parse(mnemonic.trim()).map_err(|_| RestoreError::InvalidMnemonic)?;
        let recovered = crate::restore::recover(&identity, self.archive.as_ref()).await?;
        crate::restore::install(&self.db, &self.process, &identity, &recovered).await?;
        Ok(recovered)
    }
}

fn observed_status(
    authorizations: &[HolderAuthorizationEnvelope],
    checked_at: Option<u64>,
) -> OnboardingStatus {
    if authorizations.is_empty() {
        OnboardingStatus::NotObserved {
            checked_at: checked_at.unwrap_or_else(now_secs),
        }
    } else {
        let mut holders = authorizations
            .iter()
            .map(|authorization| {
                authorization
                    .holder_authorization
                    .authorization
                    .holder_id_pubkey
                    .0
            })
            .collect::<Vec<_>>();
        holders.sort_by_key(ToString::to_string);
        holders.dedup();
        OnboardingStatus::AuthorizationObserved {
            authorizations: authorizations.len(),
            holders,
            checked_at,
        }
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock after the epoch")
        .as_secs()
}

/// What the completed stage answers while the fleet has not opened yet.
///
/// The onboarding verbs report what a running fleet reports, because the
/// question they ask has been settled. Everything else reports neither the
/// running answer (there is no fleet) nor `NotOnboarded` (onboarding is done):
/// saying "not onboarded" here would send a browser that just finished the
/// wizard back to its first screen.
fn already_completed(request: &AdminRequest) -> anyhow::Result<Value> {
    match request {
        AdminRequest::OnboardAsNew { if_needed: true } => {
            Ok(crate::admin::onboarded_already_json())
        }
        AdminRequest::OnboardAsNew { .. } | AdminRequest::OnboardFromBackup { .. } => {
            Err(RestoreError::AlreadyOnboarded.into())
        }
        _ => Err(anyhow::anyhow!(
            "this Fleet Manager has completed onboarding and is starting; its fleet is not open yet"
        )),
    }
}

/// A host that has just minted its identity starts with no seats, so the count
/// is a constant rather than a read.
pub fn onboarded_new_json() -> Value {
    json!({ "onboarded": "new", "seats": 0 })
}

/// A restore reports what came back off the relay: how many seats the phrase
/// recovered, and how many of them had already formed a federation.
pub fn onboarded_restored_json(seats: usize, formed: usize) -> Value {
    json!({
        "onboarded": "restored",
        "seats": seats,
        "formed": formed,
    })
}

#[cfg(test)]
#[path = "../tests/onboarding.rs"]
mod tests;
