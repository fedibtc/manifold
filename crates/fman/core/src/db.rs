mod seats;

pub(crate) use seats::SeatAdmissionResult;
pub(crate) use seats::{CompletionCallbackOutcome, CompletionCallbackRecord};
pub use seats::{
    NewPayment, NewSeat, PaymentRecord, RestoredGuardianConfig, RestoredSeat, SeatRecord,
};

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::wallet::Msats;
use fedi_decentralized_manifold_environment::ManifoldEnvironment;
use fedi_decentralized_service_fleet_manager::{FederationId, OfferEpoch, Plan};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::{Sqlite, SqlitePool, Transaction};
use thiserror::Error;

use crate::identity::RootMnemonic;
use fedi_decentralized_service_fleet_manager::SeatId;

pub(crate) const IDENTITY_ID: i64 = 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OnboardingStage {
    Identity,
    HolderAuthorization,
    InitialOffer,
    Complete,
}

impl OnboardingStage {
    fn parse(stage: &str) -> Result<Self, DbError> {
        match stage {
            "identity" => Ok(Self::Identity),
            "holder_authorization" => Ok(Self::HolderAuthorization),
            "initial_offer" => Ok(Self::InitialOffer),
            "complete" => Ok(Self::Complete),
            _ => Err(DbError::CorruptRow {
                table: "onboarding_state",
                key: "1".to_owned(),
                detail: format!("unknown stage {stage:?}"),
            }),
        }
    }
}

/// Quoting inputs, read and changed with their epoch. The price is the
/// operator's; the accepted payment federations are the membership of the
/// admitted common setup-payment set, written only by
/// [`Db::replace_setup_payment_policy`].
///
/// Storage is opinionated where the wire is general: the wire carries a list
/// of [`Plan`]s so the vocabulary can grow, but this daemon can serve exactly
/// one of them, so what it stores — and what an operator sets — is the price
/// it sells at, or nothing, meaning it is not selling.
/// [`QuoteSettings::plans`] states that price the way the wire does.
#[derive(Clone, PartialEq)]
pub(crate) struct QuoteSettings {
    pub(crate) price: Option<Msats>,
    pub(crate) payment_federations: Vec<FederationId>,
}

impl QuoteSettings {
    /// The offer as the wire states it: the stored price as the single plan
    /// this daemon serves, or an empty offer.
    pub(crate) fn plans(&self) -> Vec<Plan> {
        self.price
            .map(|price| Plan::InfiniteBestEffort {
                price_msats: price.0,
            })
            .into_iter()
            .collect()
    }
}

#[derive(Clone)]
pub(crate) struct Offer {
    pub(crate) epoch: OfferEpoch,
    pub(crate) settings: QuoteSettings,
}

pub(crate) struct OfferSnapshot {
    pub(crate) offer: Offer,
    pub(crate) slots: u32,
}

#[derive(Clone, Debug)]
pub struct Db {
    pool: SqlitePool,
    /// Exclusive single-instance lock for this data root. Keeping it beside
    /// the pool makes every database holder retain the root through detached
    /// cleanup.
    _data_root_lock: Arc<std::fs::File>,
}

impl Db {
    /// The durable generation for the one FMan-wide telemetry capability.
    pub(crate) async fn telemetry_capability_generation(&self) -> Result<u64, DbError> {
        let generation: i64 =
            sqlx::query_scalar("SELECT generation FROM telemetry_capability WHERE id = 1")
                .fetch_one(&self.pool)
                .await?;
        u64::try_from(generation).map_err(|_| DbError::CorruptRow {
            table: "telemetry_capability",
            key: "1".to_owned(),
            detail: "generation is outside the u64 domain".into(),
        })
    }

    /// Rotate the global capability and return its new durable generation.
    pub(crate) async fn rotate_telemetry_capability_generation(&self) -> Result<u64, DbError> {
        let generation: Option<i64> = sqlx::query_scalar(
            "UPDATE telemetry_capability \
             SET generation = generation + 1, updated_at_ms = ? \
             WHERE id = 1 AND generation < 9223372036854775807 \
             RETURNING generation",
        )
        .bind(now_ms())
        .fetch_optional(&self.pool)
        .await?;
        let generation = generation.ok_or(DbError::TelemetryGenerationExhausted)?;
        u64::try_from(generation).map_err(|_| DbError::CorruptRow {
            table: "telemetry_capability",
            key: "1".to_owned(),
            detail: "generation is outside the u64 domain".into(),
        })
    }

    /// Read epoch, quote settings, and capacity in one SQLite snapshot.
    pub(crate) async fn offer_snapshot(
        &self,
        first_port_base: crate::facts::PortBase,
    ) -> Result<OfferSnapshot, DbError> {
        let mut tx = self.pool.begin().await?;
        let payment_rows: Vec<(String,)> = sqlx::query_as(
            "SELECT federation_id FROM setup_payment_federation_members ORDER BY federation_id",
        )
        .fetch_all(&mut *tx)
        .await?;
        let payment_federations = payment_rows
            .into_iter()
            .map(|(id,)| FederationId(id))
            .collect();
        let (epoch, price, max_seats): (Vec<u8>, Option<i64>, i64) = sqlx::query_as(
            "SELECT offer_epoch, price_msats, max_seats FROM offer_state WHERE id = 1",
        )
        .fetch_one(&mut *tx)
        .await?;
        let (active, next_no): (i64, i64) = sqlx::query_as(
            "SELECT COUNT(*) FILTER (WHERE d.quote_id IS NULL), COALESCE(MAX(s.seat_no) + 1, 0) \
             FROM seats s LEFT JOIN decommissioned_seats d USING (quote_id)",
        )
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        let epoch = parse_offer_epoch(epoch)?;
        Ok(OfferSnapshot {
            offer: Offer {
                epoch,
                settings: QuoteSettings {
                    price: price.map(stored_price),
                    payment_federations,
                },
            },
            slots: available_slots(
                stored_max_seats(max_seats)?,
                first_port_base,
                active,
                next_no,
            ),
        })
    }

    pub async fn open(data_root: &Path) -> Result<Self, DbError> {
        let data_root_lock = Arc::new(lock_data_root(data_root).await?);
        let path = data_root.join("fleet-manager.sqlite");
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .foreign_keys(true)
            // REPLACE implements a uniqueness conflict as an implicit DELETE.
            // Make the schema's no-delete trigger cover that path on every
            // pooled connection as well as explicit DELETE statements.
            .pragma("recursive_triggers", "ON")
            .journal_mode(SqliteJournalMode::Wal)
            // FULL: commits fsync before returning. Rows here are promises
            // to external parties — signed commitments already sent to an
            // FI, and monotone counter updates that make refusals permanent —
            // and must survive power loss (ARCH-fleet-manager-storage).
            .synchronous(SqliteSynchronous::Full)
            .busy_timeout(Duration::from_secs(5));
        let pool = SqlitePoolOptions::new().connect_with(options).await?;

        sqlx::migrate!("./migrations").run(&pool).await?;

        Ok(Self {
            pool,
            _data_root_lock: data_root_lock,
        })
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// Begin a transaction at SQLite's single-writer linearization point.
    /// Deciding reads inside it therefore cannot race another offer writer.
    pub(super) async fn begin_write(&self) -> Result<Transaction<'static, Sqlite>, DbError> {
        Ok(self.pool.begin_with("BEGIN IMMEDIATE").await?)
    }

    /// Bind this data root to one Manifold environment, or confirm its existing
    /// binding. The insert is conflict-safe so concurrent first starts can only
    /// establish one winner; every other environment then fails before using
    /// any identity, policy, wallet, or seat state from this database.
    pub async fn bind_manifold_environment(
        &self,
        selected: ManifoldEnvironment,
    ) -> Result<(), DbError> {
        let selected_name = selected.to_string();
        sqlx::query(
            "INSERT INTO manifold_environment (id, environment) \
             SELECT 1, ? WHERE NOT EXISTS (SELECT 1 FROM manifold_environment)",
        )
        .bind(&selected_name)
        .execute(&self.pool)
        .await?;
        let bound: String =
            sqlx::query_scalar("SELECT environment FROM manifold_environment WHERE id = 1")
                .fetch_one(&self.pool)
                .await?;
        if bound != selected_name {
            return Err(DbError::ManifoldEnvironmentMismatch { bound, selected });
        }
        Ok(())
    }

    pub async fn load_identity(&self) -> Result<Option<RootMnemonic>, DbError> {
        Ok(
            sqlx::query!("SELECT mnemonic FROM identity WHERE id = ?", IDENTITY_ID)
                .fetch_optional(&self.pool)
                .await?
                .map(|row| RootMnemonic::parse(&row.mnemonic))
                .transpose()?,
        )
    }

    pub async fn onboarding_stage(&self) -> Result<OnboardingStage, DbError> {
        let stage: String = sqlx::query_scalar("SELECT stage FROM onboarding_state WHERE id = 1")
            .fetch_one(&self.pool)
            .await?;
        OnboardingStage::parse(&stage)
    }

    /// Write the identity onboarding chose, once.
    ///
    /// A Fleet Manager has exactly one mnemonic and acquires it exactly once,
    /// at onboarding: either freshly generated, or the phrase a restore
    /// recovered from ([`crate::onboarding`]). Nothing else creates one — a
    /// daemon that finds no identity row has not begun onboarding and does not
    /// invent one for itself, because a host that was going to be recovered
    /// would then be carrying a mnemonic nobody asked for.
    ///
    /// A second call fails on the primary key, which is the enforcement of
    /// "onboarding happens once".
    pub async fn install_identity(&self, identity: &RootMnemonic) -> Result<(), DbError> {
        let phrase = identity.phrase();
        let created_at_ms = now_ms();
        let mut tx = self.begin_write().await?;
        let stage: String = sqlx::query_scalar("SELECT stage FROM onboarding_state WHERE id = 1")
            .fetch_one(&mut *tx)
            .await?;
        let stage = OnboardingStage::parse(&stage)?;
        if stage != OnboardingStage::Identity {
            return Err(DbError::WrongOnboardingStage {
                expected: OnboardingStage::Identity,
                actual: stage,
            });
        }
        sqlx::query("INSERT INTO identity (id, mnemonic, created_at_ms) VALUES (?, ?, ?)")
            .bind(IDENTITY_ID)
            .bind(phrase)
            .bind(created_at_ms)
            .execute(&mut *tx)
            .await?;
        let stage_update = sqlx::query(
            "UPDATE onboarding_state SET stage = 'holder_authorization', updated_at_ms = ? \
             WHERE id = 1 AND stage = 'identity'",
        )
        .bind(created_at_ms)
        .execute(&mut *tx)
        .await?;
        if stage_update.rows_affected() != 1 {
            return Err(DbError::WrongOnboardingStage {
                expected: OnboardingStage::Identity,
                actual: stage,
            });
        }
        tx.commit().await?;
        Ok(())
    }

    pub(crate) async fn payout_destination(&self) -> Result<Option<String>, DbError> {
        Ok(
            sqlx::query_scalar("SELECT destination FROM payout_settings WHERE id = 1")
                .fetch_one(&self.pool)
                .await?,
        )
    }

    pub(crate) async fn set_payout_destination(
        &self,
        destination: Option<&str>,
    ) -> Result<(), DbError> {
        sqlx::query("UPDATE payout_settings SET destination = ? WHERE id = 1")
            .bind(destination)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Replace the whole offer (the offer is one fact). A changed offer draws
    /// a fresh epoch, permanently refusing every quote priced under the old
    /// one; an unchanged offer keeps the epoch, so a redundant set does not
    /// invalidate quotes in flight.
    pub(crate) async fn set_offered_price(
        &self,
        price: Option<Msats>,
    ) -> Result<OfferEpoch, DbError> {
        let price_msats = price.map(|price| price.0 as i64);
        let updated_at_ms = now_ms();
        let mut tx = self.begin_write().await?;
        let existing = sqlx::query_scalar::<_, Option<i64>>(
            "SELECT price_msats FROM offer_state WHERE id = 1",
        )
        .fetch_one(&mut *tx)
        .await?;
        if existing != price_msats {
            // A CSPRNG draw prevents a refused epoch from recurring after a
            // restore; do not replace it with a derived or sequential value.
            sqlx::query(
                "UPDATE offer_state SET price_msats = ?, updated_at_ms = ?, offer_epoch = ? \
                 WHERE id = 1",
            )
            .bind(price_msats)
            .bind(updated_at_ms)
            .bind(fresh_offer_epoch().as_bytes().as_slice())
            .execute(&mut *tx)
            .await?;
        }
        let epoch =
            sqlx::query_scalar::<_, Vec<u8>>("SELECT offer_epoch FROM offer_state WHERE id = 1")
                .fetch_one(&mut *tx)
                .await?;
        tx.commit().await?;
        parse_offer_epoch(epoch)
    }

    /// Store the first complete operator offer and finish setup. Holder
    /// authorization has already been retained before this transition.
    pub async fn configure_initial_offer(
        &self,
        price: Option<Msats>,
        max_seats: u32,
    ) -> Result<(), DbError> {
        let price_msats = price.map(|price| price.0 as i64);
        let max_seats_i64 = i64::from(max_seats);
        let updated_at_ms = now_ms();
        let mut tx = self.begin_write().await?;
        let stage: String = sqlx::query_scalar("SELECT stage FROM onboarding_state WHERE id = 1")
            .fetch_one(&mut *tx)
            .await?;
        if OnboardingStage::parse(&stage)? != OnboardingStage::InitialOffer {
            return Err(DbError::WrongOnboardingStage {
                expected: OnboardingStage::InitialOffer,
                actual: OnboardingStage::parse(&stage)?,
            });
        }
        let active: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM seats s LEFT JOIN decommissioned_seats d USING (quote_id) \
             WHERE d.quote_id IS NULL",
        )
        .fetch_one(&mut *tx)
        .await?;
        let active = u32::try_from(active).unwrap_or(u32::MAX);
        if max_seats < active {
            return Err(DbError::SeatLimitBelowActive {
                requested: max_seats,
                active,
            });
        }
        sqlx::query(
            "UPDATE offer_state SET price_msats = ?, max_seats = ?, updated_at_ms = ?, \
             offer_epoch = ? WHERE id = 1",
        )
        .bind(price_msats)
        .bind(max_seats_i64)
        .bind(updated_at_ms)
        .bind(fresh_offer_epoch().as_bytes().as_slice())
        .execute(&mut *tx)
        .await?;
        let stage_update = sqlx::query(
            "UPDATE onboarding_state SET stage = 'complete', updated_at_ms = ? WHERE id = 1",
        )
        .bind(updated_at_ms)
        .execute(&mut *tx)
        .await?;
        if stage_update.rows_affected() != 1 {
            return Err(DbError::WrongOnboardingStage {
                expected: OnboardingStage::InitialOffer,
                actual: OnboardingStage::parse(&stage)?,
            });
        }
        tx.commit().await?;
        Ok(())
    }

    /// Change admission capacity without allowing the configured ceiling to
    /// move below seats that are still active.
    pub(crate) async fn set_max_seats(&self, max_seats: u32) -> Result<(), DbError> {
        let mut tx = self.begin_write().await?;
        let active: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM seats s LEFT JOIN decommissioned_seats d USING (quote_id) \
             WHERE d.quote_id IS NULL",
        )
        .fetch_one(&mut *tx)
        .await?;
        let active = u32::try_from(active).unwrap_or(u32::MAX);
        if max_seats < active {
            return Err(DbError::SeatLimitBelowActive {
                requested: max_seats,
                active,
            });
        }
        let existing: i64 = sqlx::query_scalar("SELECT max_seats FROM offer_state WHERE id = 1")
            .fetch_one(&mut *tx)
            .await?;
        if stored_max_seats(existing)? != max_seats {
            sqlx::query(
                "UPDATE offer_state SET max_seats = ?, updated_at_ms = ?, offer_epoch = ? \
                 WHERE id = 1",
            )
            .bind(i64::from(max_seats))
            .bind(now_ms())
            .bind(fresh_offer_epoch().as_bytes().as_slice())
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    pub(crate) async fn max_seats(&self) -> Result<u32, DbError> {
        let max_seats: i64 = sqlx::query_scalar("SELECT max_seats FROM offer_state WHERE id = 1")
            .fetch_one(&self.pool)
            .await?;
        stored_max_seats(max_seats)
    }

    pub async fn active_seat_count(&self) -> Result<u32, DbError> {
        let active: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM seats s LEFT JOIN decommissioned_seats d USING (quote_id) \
             WHERE d.quote_id IS NULL",
        )
        .fetch_one(&self.pool)
        .await?;
        u32::try_from(active).map_err(|_| DbError::CorruptRow {
            table: "seats",
            key: "active-count".to_owned(),
            detail: "active seat count is outside the u32 domain".to_owned(),
        })
    }

    #[cfg(test)]
    pub(crate) async fn complete_onboarding_for_test(&self, max_seats: u32) -> Result<(), DbError> {
        let mut tx = self.begin_write().await?;
        sqlx::query("UPDATE offer_state SET max_seats = ?, updated_at_ms = ? WHERE id = 1")
            .bind(i64::from(max_seats))
            .bind(now_ms())
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "UPDATE onboarding_state SET stage = 'complete', updated_at_ms = ? WHERE id = 1",
        )
        .bind(now_ms())
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// The retained common setup-payment publication (the complete Nostr
    /// event), or `None` before the first admission. The Nostr boundary
    /// statically revalidates it after restart and uses it as the NIP-01
    /// high-water mark.
    pub(crate) async fn setup_payment_event_json(&self) -> Result<Option<String>, DbError> {
        Ok(
            sqlx::query!("SELECT event_json FROM nostr_setup_payment_federations WHERE id = 1")
                .fetch_optional(&self.pool)
                .await?
                .map(|row| row.event_json),
        )
    }

    /// Normalize and load Holder-authorization events accepted during
    /// operator-driven enrollment.
    ///
    /// This removes pre-fix rows outside the current time and aggregate bounds
    /// before the Nostr boundary revalidates every returned event.
    pub(crate) async fn bounded_holder_authorization_event_jsons(
        &self,
        max_issued_at: u64,
    ) -> Result<Vec<String>, DbError> {
        let mut tx = self.begin_write().await?;
        sqlx::query("DELETE FROM holder_authorization_events WHERE authorization_issued_at > ?")
            .bind(max_issued_at.to_be_bytes().to_vec())
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "DELETE FROM holder_authorization_events
             WHERE credential_digest NOT IN (
                 SELECT credential_digest FROM holder_authorization_events
                 ORDER BY credential_digest
                 LIMIT ?
             )",
        )
        .bind(
            i64::try_from(fedi_decentralized_domain::FMAN_HOLDER_AUTHORIZATION_RETENTION_MAX_COUNT)
                .expect("Holder authorization retention bound fits SQLite INTEGER"),
        )
        .execute(&mut *tx)
        .await?;
        let events = sqlx::query_scalar(
            "SELECT event_json FROM holder_authorization_events ORDER BY credential_digest",
        )
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(events)
    }

    /// Merge verified Holder authorizations without treating relay omission as
    /// deletion. A later authorization for the same credential supersedes an
    /// earlier one; equal or older statements cannot roll retained state back.
    /// New credential digests are ignored once this FMan identity's aggregate
    /// retention bound is full; existing rows are never evicted for relay churn.
    pub(crate) async fn merge_holder_authorization_events(
        &self,
        events: &[(Vec<u8>, u64, String)],
        max_issued_at: u64,
    ) -> Result<(), DbError> {
        if events
            .iter()
            .any(|(_, authorization_issued_at, _)| *authorization_issued_at > max_issued_at)
        {
            return Err(DbError::HolderAuthorizationIssuedAtTooFarFuture);
        }
        let mut tx = self.begin_write().await?;
        sqlx::query("DELETE FROM holder_authorization_events WHERE authorization_issued_at > ?")
            .bind(max_issued_at.to_be_bytes().to_vec())
            .execute(&mut *tx)
            .await?;
        for (credential_digest, authorization_issued_at, event_json) in events {
            let authorization_issued_at = authorization_issued_at.to_be_bytes().to_vec();
            sqlx::query(
                "INSERT INTO holder_authorization_events
                 (credential_digest, authorization_issued_at, event_json)
                 SELECT ?, ?, ?
                 WHERE EXISTS (
                     SELECT 1 FROM holder_authorization_events WHERE credential_digest = ?
                 ) OR (
                     SELECT COUNT(*) FROM holder_authorization_events
                 ) < ?
                 ON CONFLICT (credential_digest) DO UPDATE SET \
                 authorization_issued_at = excluded.authorization_issued_at, \
                 event_json = excluded.event_json \
                 WHERE excluded.authorization_issued_at > \
                       holder_authorization_events.authorization_issued_at",
            )
            .bind(credential_digest)
            .bind(authorization_issued_at)
            .bind(event_json)
            .bind(credential_digest)
            .bind(
                i64::try_from(
                    fedi_decentralized_domain::FMAN_HOLDER_AUTHORIZATION_RETENTION_MAX_COUNT,
                )
                .expect("Holder authorization retention bound fits SQLite INTEGER"),
            )
            .execute(&mut *tx)
            .await?;
        }
        let retained: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM holder_authorization_events")
            .fetch_one(&mut *tx)
            .await?;
        if retained > 0 {
            let stage: String =
                sqlx::query_scalar("SELECT stage FROM onboarding_state WHERE id = 1")
                    .fetch_one(&mut *tx)
                    .await?;
            let stage = OnboardingStage::parse(&stage)?;
            let stage_update = sqlx::query(
                "UPDATE onboarding_state SET stage = 'initial_offer', updated_at_ms = ? \
                 WHERE id = 1 AND stage = 'holder_authorization'",
            )
            .bind(now_ms())
            .execute(&mut *tx)
            .await?;
            if stage == OnboardingStage::HolderAuthorization && stage_update.rows_affected() != 1 {
                return Err(DbError::WrongOnboardingStage {
                    expected: OnboardingStage::HolderAuthorization,
                    actual: stage,
                });
            }
        }
        tx.commit().await?;
        Ok(())
    }

    /// Atomically replace the setup-payment policy: retain the admitted
    /// event, replace the derived accepted membership, and — when any
    /// previously accepted member is no longer in the set — draw a fresh
    /// offer epoch, all in one transaction. Every outstanding quote against
    /// a removed federation is thereby refused (with its refund) rather than
    /// settled; additions leave in-flight quotes valid.
    pub(crate) async fn replace_setup_payment_policy(
        &self,
        event_json: &str,
        member_ids: &[FederationId],
    ) -> Result<(), DbError> {
        let mut tx = self.begin_write().await?;
        let previous: Vec<String> =
            sqlx::query_scalar("SELECT federation_id FROM setup_payment_federation_members")
                .fetch_all(&mut *tx)
                .await?;
        sqlx::query!(
            "INSERT INTO nostr_setup_payment_federations (id, event_json) VALUES (1, ?) \
             ON CONFLICT (id) DO UPDATE SET event_json = excluded.event_json",
            event_json,
        )
        .execute(&mut *tx)
        .await?;
        sqlx::query("DELETE FROM setup_payment_federation_members")
            .execute(&mut *tx)
            .await?;
        for member in member_ids {
            sqlx::query("INSERT INTO setup_payment_federation_members (federation_id) VALUES (?)")
                .bind(&member.0)
                .execute(&mut *tx)
                .await?;
        }
        let removed_any = previous
            .iter()
            .any(|previous_id| !member_ids.iter().any(|member| member.0 == *previous_id));
        if removed_any {
            sqlx::query("UPDATE offer_state SET offer_epoch = ? WHERE id = 1")
                .bind(fresh_offer_epoch().as_bytes().as_slice())
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    pub async fn offer_epoch(&self) -> Result<OfferEpoch, DbError> {
        let (epoch,): (Vec<u8>,) =
            sqlx::query_as("SELECT offer_epoch FROM offer_state WHERE id = 1")
                .fetch_one(&self.pool)
                .await?;
        parse_offer_epoch(epoch)
    }
}

pub(super) fn fresh_offer_epoch() -> OfferEpoch {
    // A CSPRNG draw prevents a refused epoch from recurring after a restore;
    // do not replace it with a derived or sequential value.
    OfferEpoch::from_bytes(rand::random())
}

pub(super) fn available_slots(
    max_seats: u32,
    first_port_base: crate::facts::PortBase,
    active: i64,
    next_no: i64,
) -> u32 {
    let seat_slots = max_seats.saturating_sub(u32::try_from(active).unwrap_or(u32::MAX));
    let next_no = u32::try_from(next_no).unwrap_or(u32::MAX);
    let port_slots = (0..seat_slots)
        .take_while(|offset| {
            crate::facts::SeatNo(next_no.saturating_add(*offset))
                .port_base(first_port_base)
                .is_some()
        })
        .count() as u32;
    seat_slots.min(port_slots)
}

/// SQLite has no unsigned integer, so a price comes back signed; the schema
/// CHECK is what keeps it non-negative.
fn stored_price(msats: i64) -> Msats {
    Msats(u64::try_from(msats).expect("price_msats is non-negative by schema CHECK"))
}

fn stored_max_seats(max_seats: i64) -> Result<u32, DbError> {
    u32::try_from(max_seats).map_err(|_| DbError::CorruptRow {
        table: "offer_state",
        key: "1".into(),
        detail: "max_seats is outside the u32 domain".into(),
    })
}

pub(super) fn parse_offer_epoch(bytes: Vec<u8>) -> Result<OfferEpoch, DbError> {
    let bytes: [u8; 32] = bytes.try_into().map_err(|_| DbError::CorruptRow {
        table: "offer_state",
        key: "1".into(),
        detail: "offer_epoch is not 32 bytes".into(),
    })?;
    Ok(OfferEpoch::from_bytes(bytes))
}

#[derive(Debug, Error)]
pub enum DbError {
    #[error(transparent)]
    DataRootLock(#[from] anyhow::Error),
    #[error("database error: {0}")]
    Sqlx(#[from] sqlx::Error),
    #[error("database migration error: {0}")]
    Migration(#[from] sqlx::migrate::MigrateError),
    #[error("identity mnemonic error: {0}")]
    Mnemonic(#[from] bip39::Error),
    #[error("data root is bound to the {bound} Manifold environment, not selected {selected}")]
    ManifoldEnvironmentMismatch {
        bound: String,
        selected: ManifoldEnvironment,
    },
    #[error("cannot set max seats to {requested}; {active} seats are active")]
    SeatLimitBelowActive { requested: u32, active: u32 },
    #[error("onboarding operation requires {expected:?}, but the current stage is {actual:?}")]
    WrongOnboardingStage {
        expected: OnboardingStage,
        actual: OnboardingStage,
    },
    #[error("corrupt {table} row {key}: {detail}")]
    CorruptRow {
        table: &'static str,
        key: String,
        detail: String,
    },
    #[error("seat {seat_id} not found")]
    SeatNotFound { seat_id: SeatId },
    #[error("telemetry capability generation is exhausted")]
    TelemetryGenerationExhausted,
    #[error("Holder authorization issue time exceeds the receiver limit")]
    HolderAuthorizationIssuedAtTooFarFuture,
    #[error("telemetry capability for seat {seat_id} is disabled")]
    TelemetryCapabilityDisabled { seat_id: SeatId },
}

/// Take the single-instance lock, or fail if another daemon holds it
/// (SPEC-admin-socket: one data root belongs to at most one running daemon).
///
/// The lock is acquired before SQLite is opened or migrated and is retained
/// by every [`Db`] clone, spanning onboarding, fleet operation, and detached
/// cleanup that still owns a database handle.
///
/// Contention is retried briefly before failing: any process fork elsewhere
/// in the program (a seat child spawn, a test harness) transiently duplicates
/// the previous holder's lock fd until its exec closes it (CLOEXEC), so a
/// close-then-reopen can race a stranger's fork window. A genuine second
/// instance holds the lock indefinitely and still fails.
async fn lock_data_root(data_root: &Path) -> anyhow::Result<std::fs::File> {
    let path = data_root.join("fleet-manager.lock");
    let file = std::fs::File::create(&path)
        .map_err(|err| anyhow::anyhow!("create lock file {}: {err}", path.display()))?;
    for _ in 0..20 {
        match file.try_lock() {
            Ok(()) => return Ok(file),
            Err(std::fs::TryLockError::WouldBlock) => {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            Err(std::fs::TryLockError::Error(err)) => {
                return Err(anyhow::Error::new(err).context(format!("lock {}", path.display())));
            }
        }
    }
    Err(anyhow::anyhow!(
        "another Fleet Manager instance already runs on {}",
        data_root.display(),
    ))
}

pub(crate) fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after Unix epoch")
        .as_millis()
        .try_into()
        .expect("current Unix timestamp fits in i64 milliseconds")
}

#[cfg(test)]
#[path = "../tests/db.rs"]
mod tests;
