use sqlx::{Any, AnyPool, Row, Transaction};

use fedi_decentralized_push_gateway_types::{
    DeviceInstallationId, FcmRegistrationToken, Platform, PushRegistration, RecipientId,
    RegisterInstallationRequest,
};

use crate::time::unix_timestamp;

/// Storage-owned definition of an installation that may receive new work.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RegistrationEligibility {
    /// Registrations older than this timestamp are stale.
    pub cutoff_timestamp: i64,
}

/// Durable admission ceilings applied in the registration transaction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RegistrationAdmissionLimits {
    /// Maximum active rows owned by one recipient; zero disables the limit.
    pub max_active_per_recipient: u64,
    /// Maximum active rows across the gateway; zero disables the limit.
    pub max_active_global: u64,
    /// Maximum physical registration and token-ownership rows; zero disables the limit.
    pub max_total_rows: u64,
    /// Maximum stale rows reclaimed by this admission attempt.
    pub reclamation_batch_size: u64,
}

/// Result of an atomic registration admission attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RegistrationAdmissionOutcome {
    /// The installation was inserted, refreshed, or moved within the same recipient.
    Registered,
    /// The recipient's active installation ceiling would be exceeded.
    RecipientCapacityExceeded,
    /// The global active installation ceiling would be exceeded.
    GlobalCapacityExceeded,
    /// The physical registration-row ceiling would be exceeded after bounded GC.
    GlobalRowCapacityExceeded,
    /// The FCM token is durably bound to a different stable installation id.
    TokenBoundToDifferentInstallation,
}

/// Low-cardinality physical registration-row metrics.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RegistrationRowMetrics {
    /// Total physical registration plus durable token-ownership rows.
    pub total: i64,
    /// Physical installation registration rows.
    pub registrations: i64,
    /// Durable token-ownership rows.
    pub token_owners: i64,
    /// Token owners whose refreshable registration row has been reclaimed.
    pub orphaned_token_owners: i64,
    /// Active, non-stale rows at the supplied cutoff.
    pub active: i64,
    /// Disabled rows.
    pub disabled: i64,
    /// Non-disabled rows older than the supplied cutoff.
    pub stale: i64,
}

/// Persistence for push notification registrations.
#[derive(Clone, Debug)]
pub struct PushRegistrationRepository {
    pool: AnyPool,
}

impl PushRegistrationRepository {
    /// Creates a repository using the given database pool.
    #[must_use]
    pub fn new(pool: AnyPool) -> Self {
        Self { pool }
    }

    /// Atomically reclaims stale rows, checks ownership/net-delta ceilings, and
    /// inserts or refreshes an installation.
    ///
    /// A globally unique FCM token may move between installation ids only when
    /// both rows belong to the same authenticated recipient. Because Fedi keeps
    /// both its FCM token and stable installation id across an account switch,
    /// every valid signed request for that exact pair is authoritative. Serialized
    /// commits use latest-valid-wins ownership and atomically update both durable
    /// rows, so another live clone may take the pair back later. A different
    /// installation id remains an ownership conflict.
    pub async fn admit_installation(
        &self,
        recipient_id: &RecipientId,
        request: &RegisterInstallationRequest,
        eligibility: RegistrationEligibility,
        limits: RegistrationAdmissionLimits,
    ) -> Result<RegistrationAdmissionOutcome, sqlx::Error> {
        let now = unix_timestamp();
        let mut transaction = self.pool.begin().await?;

        acquire_admission_lock(&mut transaction, "registration", now).await?;

        purge_stale_in_transaction(
            &mut transaction,
            eligibility.cutoff_timestamp,
            limits.reclamation_batch_size,
        )
        .await?;

        let token_owner = sqlx::query(
            "SELECT recipient_id, installation_id
             FROM push_registration_token_owners WHERE fcm_token = $1",
        )
        .bind(&request.fcm_token.0)
        .fetch_optional(&mut *transaction)
        .await?;
        let token_registration = sqlx::query(
            "SELECT recipient_id, installation_id, disabled_at, last_seen_at
             FROM push_registrations WHERE fcm_token = $1",
        )
        .bind(&request.fcm_token.0)
        .fetch_optional(&mut *transaction)
        .await?;
        let owner_conflicts_with_installation = token_owner.as_ref().is_some_and(|row| {
            row.get::<String, _>("recipient_id") != recipient_id.0
                && row.get::<String, _>("installation_id") != request.installation_id.0
        });
        let registration_conflicts_with_installation =
            token_registration.as_ref().is_some_and(|row| {
                row.get::<String, _>("recipient_id") != recipient_id.0
                    && row.get::<String, _>("installation_id") != request.installation_id.0
            });
        if owner_conflicts_with_installation || registration_conflicts_with_installation {
            transaction.rollback().await?;
            return Ok(RegistrationAdmissionOutcome::TokenBoundToDifferentInstallation);
        }

        let target = sqlx::query(
            "SELECT fcm_token, disabled_at, last_seen_at FROM push_registrations
             WHERE recipient_id = $1 AND installation_id = $2",
        )
        .bind(&recipient_id.0)
        .bind(&request.installation_id.0)
        .fetch_optional(&mut *transaction)
        .await?;
        let target_is_active = target
            .as_ref()
            .is_some_and(|row| registration_row_is_active(row, eligibility));
        let token_registration_is_other_route = token_registration.as_ref().is_some_and(|row| {
            row.get::<String, _>("recipient_id") != recipient_id.0
                || row.get::<String, _>("installation_id") != request.installation_id.0
        });
        let token_registration_is_active = token_registration
            .as_ref()
            .is_some_and(|row| registration_row_is_active(row, eligibility));
        let token_registration_belongs_to_recipient = token_registration
            .as_ref()
            .is_some_and(|row| row.get::<String, _>("recipient_id") == recipient_id.0);
        let removed_recipient_active = u64::from(target_is_active)
            + u64::from(
                token_registration_is_other_route
                    && token_registration_belongs_to_recipient
                    && token_registration_is_active,
            );
        let removed_global_active = u64::from(target_is_active)
            + u64::from(token_registration_is_other_route && token_registration_is_active);
        let target_old_token = target.as_ref().map(|row| row.get::<String, _>("fcm_token"));
        let releases_target_old_owner = if let Some(old_token) = target_old_token
            .as_deref()
            .filter(|old_token| *old_token != request.fcm_token.0)
        {
            token_owner_matches_installation(
                &mut transaction,
                old_token,
                recipient_id,
                &request.installation_id,
            )
            .await?
        } else {
            false
        };

        if 0 < limits.max_active_per_recipient {
            let count = active_registration_count(
                &mut transaction,
                Some(recipient_id),
                eligibility.cutoff_timestamp,
            )
            .await?;
            let projected = count
                .saturating_sub(removed_recipient_active)
                .saturating_add(1);
            if limits.max_active_per_recipient < projected {
                transaction.rollback().await?;
                return Ok(RegistrationAdmissionOutcome::RecipientCapacityExceeded);
            }
        }
        if 0 < limits.max_active_global {
            let count =
                active_registration_count(&mut transaction, None, eligibility.cutoff_timestamp)
                    .await?;
            let projected = count
                .saturating_sub(removed_global_active)
                .saturating_add(1);
            if limits.max_active_global < projected {
                transaction.rollback().await?;
                return Ok(RegistrationAdmissionOutcome::GlobalCapacityExceeded);
            }
        }
        if 0 < limits.max_total_rows {
            let count = physical_registration_count(&mut transaction).await?;
            let removed =
                u64::from(token_registration_is_other_route) + u64::from(releases_target_old_owner);
            let added = u64::from(target.is_none()) + u64::from(token_owner.is_none());
            let projected = count.saturating_sub(removed).saturating_add(added);
            if limits.max_total_rows < projected {
                transaction.rollback().await?;
                return Ok(RegistrationAdmissionOutcome::GlobalRowCapacityExceeded);
            }
        }

        if token_registration_is_other_route {
            sqlx::query(
                "DELETE FROM push_registrations
                 WHERE fcm_token = $1",
            )
            .bind(&request.fcm_token.0)
            .execute(&mut *transaction)
            .await?;
        }

        if releases_target_old_owner {
            sqlx::query(
                "DELETE FROM push_registration_token_owners
                 WHERE fcm_token = $1 AND recipient_id = $2 AND installation_id = $3",
            )
            .bind(target_old_token.as_deref())
            .bind(&recipient_id.0)
            .bind(&request.installation_id.0)
            .execute(&mut *transaction)
            .await?;
        }

        sqlx::query(
            "INSERT INTO push_registrations (
                 recipient_id, installation_id, fcm_token, platform, created_at,
                 updated_at, last_seen_at, disabled_at, disabled_reason
             ) VALUES ($1, $2, $3, $4, $5, $5, $5, NULL, NULL)
             ON CONFLICT(recipient_id, installation_id) DO UPDATE SET
                 fcm_token = excluded.fcm_token,
                 platform = excluded.platform,
                 updated_at = excluded.updated_at,
                 last_seen_at = excluded.last_seen_at,
                 disabled_at = NULL,
                 disabled_reason = NULL",
        )
        .bind(&recipient_id.0)
        .bind(&request.installation_id.0)
        .bind(&request.fcm_token.0)
        .bind(
            request
                .platform
                .as_ref()
                .map(|platform| platform.0.as_str()),
        )
        .bind(now)
        .execute(&mut *transaction)
        .await?;

        if token_owner.is_some() {
            sqlx::query(
                "UPDATE push_registration_token_owners
                 SET installation_id = $2, updated_at = $3, recipient_id = $4
                 WHERE fcm_token = $1",
            )
            .bind(&request.fcm_token.0)
            .bind(&request.installation_id.0)
            .bind(now)
            .bind(&recipient_id.0)
            .execute(&mut *transaction)
            .await?;
        } else {
            sqlx::query(
                "INSERT INTO push_registration_token_owners (
                     fcm_token, recipient_id, installation_id, updated_at
                 ) VALUES ($1, $2, $3, $4)",
            )
            .bind(&request.fcm_token.0)
            .bind(&recipient_id.0)
            .bind(&request.installation_id.0)
            .bind(now)
            .execute(&mut *transaction)
            .await?;
        }

        transaction.commit().await?;
        Ok(RegistrationAdmissionOutcome::Registered)
    }

    /// Returns active, non-stale registrations for one recipient.
    pub async fn list_for_recipient(
        &self,
        recipient_id: &RecipientId,
        eligibility: RegistrationEligibility,
    ) -> Result<Vec<PushRegistration>, sqlx::Error> {
        let rows = sqlx::query(
            "SELECT recipient_id, installation_id, fcm_token, platform, created_at,
                    last_seen_at, disabled_at, disabled_reason
             FROM push_registrations
             WHERE recipient_id = $1 AND disabled_at IS NULL AND $2 <= last_seen_at
             ORDER BY installation_id",
        )
        .bind(&recipient_id.0)
        .bind(eligibility.cutoff_timestamp)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(row_to_registration).collect())
    }

    /// Returns one active, non-stale installation owned by the recipient.
    pub async fn eligible_installation(
        &self,
        recipient_id: &RecipientId,
        installation_id: &DeviceInstallationId,
        eligibility: RegistrationEligibility,
    ) -> Result<Option<PushRegistration>, sqlx::Error> {
        let mut transaction = self.pool.begin().await?;
        let result = Self::eligible_installation_in_transaction(
            &mut transaction,
            recipient_id,
            installation_id,
            eligibility,
        )
        .await?;
        transaction.commit().await?;
        Ok(result)
    }

    /// Transactional variant used when eligibility must precede another durable mutation.
    pub async fn eligible_installation_in_transaction(
        transaction: &mut Transaction<'_, Any>,
        recipient_id: &RecipientId,
        installation_id: &DeviceInstallationId,
        eligibility: RegistrationEligibility,
    ) -> Result<Option<PushRegistration>, sqlx::Error> {
        let row = sqlx::query(
            "SELECT recipient_id, installation_id, fcm_token, platform, created_at,
                    last_seen_at, disabled_at, disabled_reason
             FROM push_registrations
             WHERE recipient_id = $1 AND installation_id = $2
               AND disabled_at IS NULL AND $3 <= last_seen_at",
        )
        .bind(&recipient_id.0)
        .bind(&installation_id.0)
        .bind(eligibility.cutoff_timestamp)
        .fetch_optional(&mut **transaction)
        .await?;
        Ok(row.map(row_to_registration))
    }

    /// Returns every active, non-stale installation for a recipient inside an
    /// existing transaction. This exists only for readable legacy hook rows;
    /// newly admitted FI hooks name exactly one installation.
    pub async fn eligible_installations_for_recipient_in_transaction(
        transaction: &mut Transaction<'_, Any>,
        recipient_id: &RecipientId,
        eligibility: RegistrationEligibility,
    ) -> Result<Vec<PushRegistration>, sqlx::Error> {
        let rows = sqlx::query(
            "SELECT recipient_id, installation_id, fcm_token, platform, created_at,
                    last_seen_at, disabled_at, disabled_reason
             FROM push_registrations
             WHERE recipient_id = $1 AND disabled_at IS NULL AND $2 <= last_seen_at
             ORDER BY installation_id",
        )
        .bind(&recipient_id.0)
        .bind(eligibility.cutoff_timestamp)
        .fetch_all(&mut **transaction)
        .await?;
        Ok(rows.into_iter().map(row_to_registration).collect())
    }

    /// Returns physical/eligible row counts for low-cardinality operator metrics.
    pub async fn row_metrics(
        &self,
        eligibility: RegistrationEligibility,
    ) -> Result<RegistrationRowMetrics, sqlx::Error> {
        let registration_row = sqlx::query(
            "SELECT COUNT(*) AS registrations,
                    COALESCE(SUM(CASE WHEN disabled_at IS NULL AND $1 <= last_seen_at THEN 1 ELSE 0 END), 0) AS active,
                    COALESCE(SUM(CASE WHEN disabled_at IS NOT NULL THEN 1 ELSE 0 END), 0) AS disabled,
                    COALESCE(SUM(CASE WHEN disabled_at IS NULL AND last_seen_at < $1 THEN 1 ELSE 0 END), 0) AS stale
             FROM push_registrations",
        )
        .bind(eligibility.cutoff_timestamp)
        .fetch_one(&self.pool)
        .await?;
        let ownership_row = sqlx::query(
            "SELECT COUNT(*) AS token_owners,
                    COALESCE(SUM(CASE WHEN NOT EXISTS (
                        SELECT 1 FROM push_registrations
                        WHERE push_registrations.fcm_token = push_registration_token_owners.fcm_token
                    ) THEN 1 ELSE 0 END), 0) AS orphaned_token_owners
             FROM push_registration_token_owners",
        )
        .fetch_one(&self.pool)
        .await?;
        let registrations: i64 = registration_row.get("registrations");
        let token_owners: i64 = ownership_row.get("token_owners");
        Ok(RegistrationRowMetrics {
            total: registrations.saturating_add(token_owners),
            registrations,
            token_owners,
            orphaned_token_owners: ownership_row.get("orphaned_token_owners"),
            active: registration_row.get("active"),
            disabled: registration_row.get("disabled"),
            stale: registration_row.get("stale"),
        })
    }

    /// Authenticated unregister deletes the registration and releases every
    /// token-ownership row still bound to this recipient/installation pair.
    pub async fn delete_installation(
        &self,
        recipient_id: &RecipientId,
        installation_id: &DeviceInstallationId,
    ) -> Result<bool, sqlx::Error> {
        let mut transaction = self.pool.begin().await?;
        acquire_admission_lock(&mut transaction, "registration", unix_timestamp()).await?;
        let registration_result = sqlx::query(
            "DELETE FROM push_registrations WHERE recipient_id = $1 AND installation_id = $2",
        )
        .bind(&recipient_id.0)
        .bind(&installation_id.0)
        .execute(&mut *transaction)
        .await?;
        let ownership_result = sqlx::query(
            "DELETE FROM push_registration_token_owners
             WHERE recipient_id = $1 AND installation_id = $2",
        )
        .bind(&recipient_id.0)
        .bind(&installation_id.0)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(0 < registration_result.rows_affected() || 0 < ownership_result.rows_affected())
    }

    /// Disables one registration while retaining lifecycle history.
    pub async fn disable_installation(
        &self,
        recipient_id: &RecipientId,
        installation_id: &DeviceInstallationId,
        reason: Option<&str>,
    ) -> Result<bool, sqlx::Error> {
        let result = sqlx::query(
            "UPDATE push_registrations
             SET disabled_at = COALESCE(disabled_at, $3), disabled_reason = COALESCE(disabled_reason, $4)
             WHERE recipient_id = $1 AND installation_id = $2",
        )
        .bind(&recipient_id.0)
        .bind(&installation_id.0)
        .bind(unix_timestamp())
        .bind(reason)
        .execute(&self.pool)
        .await?;
        Ok(0 < result.rows_affected())
    }

    /// Disables a registration only if its current token still matches a snapshot.
    pub async fn disable_installation_if_token_matches(
        &self,
        recipient_id: &RecipientId,
        installation_id: &DeviceInstallationId,
        fcm_token: &FcmRegistrationToken,
        reason: Option<&str>,
    ) -> Result<bool, sqlx::Error> {
        let result = sqlx::query(
            "UPDATE push_registrations
             SET disabled_at = COALESCE(disabled_at, $4), disabled_reason = COALESCE(disabled_reason, $5)
             WHERE recipient_id = $1 AND installation_id = $2 AND fcm_token = $3",
        )
        .bind(&recipient_id.0)
        .bind(&installation_id.0)
        .bind(&fcm_token.0)
        .bind(unix_timestamp())
        .bind(reason)
        .execute(&self.pool)
        .await?;
        Ok(0 < result.rows_affected())
    }

    /// Deletes stale registrations and their now-orphaned token owners.
    pub async fn purge_stale(&self, cutoff_timestamp: i64) -> Result<u64, sqlx::Error> {
        let mut transaction = self.pool.begin().await?;
        acquire_admission_lock(&mut transaction, "registration", unix_timestamp()).await?;
        let registrations = sqlx::query("DELETE FROM push_registrations WHERE last_seen_at < $1")
            .bind(cutoff_timestamp)
            .execute(&mut *transaction)
            .await?
            .rows_affected();
        let token_owners = purge_orphaned_token_owners_in_transaction(
            &mut transaction,
            cutoff_timestamp,
            u64::MAX,
        )
        .await?;
        transaction.commit().await?;
        Ok(registrations.saturating_add(token_owners))
    }
}

async fn purge_stale_in_transaction(
    transaction: &mut Transaction<'_, Any>,
    cutoff_timestamp: i64,
    batch_size: u64,
) -> Result<u64, sqlx::Error> {
    if batch_size == 0 {
        return Ok(0);
    }
    let batch_size_i64 = i64::try_from(batch_size).unwrap_or(i64::MAX);
    let registrations = sqlx::query(
        "DELETE FROM push_registrations
         WHERE (recipient_id, installation_id) IN (
             SELECT recipient_id, installation_id FROM push_registrations
             WHERE last_seen_at < $1 ORDER BY last_seen_at LIMIT $2
         )",
    )
    .bind(cutoff_timestamp)
    .bind(batch_size_i64)
    .execute(&mut **transaction)
    .await?
    .rows_affected();
    let token_owners =
        purge_orphaned_token_owners_in_transaction(transaction, cutoff_timestamp, batch_size)
            .await?;
    Ok(registrations.saturating_add(token_owners))
}

async fn purge_orphaned_token_owners_in_transaction(
    transaction: &mut Transaction<'_, Any>,
    cutoff_timestamp: i64,
    batch_size: u64,
) -> Result<u64, sqlx::Error> {
    if batch_size == 0 {
        return Ok(0);
    }
    let batch_size = i64::try_from(batch_size).unwrap_or(i64::MAX);
    Ok(sqlx::query(
        "DELETE FROM push_registration_token_owners
         WHERE fcm_token IN (
             SELECT owner.fcm_token
             FROM push_registration_token_owners owner
             WHERE owner.updated_at < $1
               AND NOT EXISTS (
                   SELECT 1 FROM push_registrations registration
                   WHERE registration.fcm_token = owner.fcm_token
               )
             ORDER BY owner.updated_at, owner.fcm_token
             LIMIT $2
         )",
    )
    .bind(cutoff_timestamp)
    .bind(batch_size)
    .execute(&mut **transaction)
    .await?
    .rows_affected())
}

async fn active_registration_count(
    transaction: &mut Transaction<'_, Any>,
    recipient_id: Option<&RecipientId>,
    cutoff_timestamp: i64,
) -> Result<u64, sqlx::Error> {
    let count: i64 = if let Some(recipient_id) = recipient_id {
        sqlx::query_scalar(
            "SELECT COUNT(*) FROM push_registrations
             WHERE recipient_id = $1 AND disabled_at IS NULL AND $2 <= last_seen_at",
        )
        .bind(&recipient_id.0)
        .bind(cutoff_timestamp)
        .fetch_one(&mut **transaction)
        .await?
    } else {
        sqlx::query_scalar(
            "SELECT COUNT(*) FROM push_registrations
             WHERE disabled_at IS NULL AND $1 <= last_seen_at",
        )
        .bind(cutoff_timestamp)
        .fetch_one(&mut **transaction)
        .await?
    };
    Ok(u64::try_from(count).unwrap_or(u64::MAX))
}

async fn physical_registration_count(
    transaction: &mut Transaction<'_, Any>,
) -> Result<u64, sqlx::Error> {
    let registration_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM push_registrations")
        .fetch_one(&mut **transaction)
        .await?;
    let owner_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM push_registration_token_owners")
            .fetch_one(&mut **transaction)
            .await?;
    Ok(u64::try_from(registration_count.saturating_add(owner_count)).unwrap_or(u64::MAX))
}

pub(crate) async fn acquire_admission_lock(
    transaction: &mut Transaction<'_, Any>,
    resource: &str,
    now_timestamp: i64,
) -> Result<(), sqlx::Error> {
    let result =
        sqlx::query("UPDATE push_gateway_admission_locks SET updated_at = $2 WHERE resource = $1")
            .bind(resource)
            .bind(now_timestamp)
            .execute(&mut **transaction)
            .await?;
    if result.rows_affected() == 1 {
        Ok(())
    } else {
        Err(sqlx::Error::RowNotFound)
    }
}

async fn token_owner_matches_installation(
    transaction: &mut Transaction<'_, Any>,
    fcm_token: &str,
    recipient_id: &RecipientId,
    installation_id: &DeviceInstallationId,
) -> Result<bool, sqlx::Error> {
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM push_registration_token_owners
         WHERE fcm_token = $1 AND recipient_id = $2 AND installation_id = $3",
    )
    .bind(fcm_token)
    .bind(&recipient_id.0)
    .bind(&installation_id.0)
    .fetch_one(&mut **transaction)
    .await?;
    Ok(count == 1)
}

fn registration_row_is_active(
    row: &sqlx::any::AnyRow,
    eligibility: RegistrationEligibility,
) -> bool {
    row.get::<Option<i64>, _>("disabled_at").is_none()
        && eligibility.cutoff_timestamp <= row.get::<i64, _>("last_seen_at")
}

fn row_to_registration(row: sqlx::any::AnyRow) -> PushRegistration {
    PushRegistration {
        recipient_id: RecipientId(row.get("recipient_id")),
        installation_id: DeviceInstallationId(row.get("installation_id")),
        fcm_token: FcmRegistrationToken(row.get("fcm_token")),
        platform: row.get::<Option<String>, _>("platform").map(Platform),
        created_at: row.get("created_at"),
        last_seen_at: row.get("last_seen_at"),
        disabled_at: row.get("disabled_at"),
        disabled_reason: row.get("disabled_reason"),
    }
}

#[cfg(test)]
mod tests;
