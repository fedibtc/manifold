//! Operator-installed trust attestations, and the issuer authorities they name.
//!
//! An attestation states who FLIP trusts to attest an FMan. It describes an
//! issuer rather than this provider, so it can be installed before a provider
//! key exists. The trust pipeline reads the installed set through
//! [`trusted_issuer_authorities`].

use fedi_decentralized_service_liquidity_manager::{
    AttestationInstallRequest, AttestationInstallResponse, AttestationKind,
    AttestationListResponse, AttestationPayloadId, AttestationPayloadInfo,
    AttestationRemoveRequest, AttestationRemoveResponse, AttestationSelector, AttestationSubject,
    AttestationSummary, IssuerAuthority, Pubkey, ServiceResult, Timestamp, canonical_json_payload,
    domain_tagged_sha256,
};
use serde::Serialize;
use serde_json::Value;
use sqlx::Row;

use crate::database::Database;
use crate::{internal_error, invalid_argument, not_found};

const PAYLOAD_ID_DOMAIN: &[u8] = b"fedi-flip-attestation-payload-id/v1\0";

pub(crate) async fn install(
    database: &Database,
    request: AttestationInstallRequest,
) -> ServiceResult<AttestationInstallResponse> {
    let parsed = parse_payload(&request.payload.0)?;
    // One authority per issuer: installing a newer document for an issuer
    // already installed replaces it rather than accumulating both.
    sqlx::query("DELETE FROM attestation_payloads WHERE kind = ? AND issuer = ? AND id != ?")
        .bind(AttestationKind::IssuerAuthority.to_string())
        .bind(parsed.issuer.as_ref().map(|issuer| issuer.0.as_str()))
        .bind(&parsed.id.0)
        .execute(database.pool())
        .await
        .map_err(internal_error)?;

    let subject_json = serde_json::to_string(&parsed.subject).map_err(internal_error)?;
    sqlx::query(
        "INSERT INTO attestation_payloads \
         (id, kind, issuer, subject_json, payload, valid, ingested_at) \
         VALUES (?, ?, ?, ?, ?, 1, unixepoch()) \
         ON CONFLICT(id) DO UPDATE SET \
           kind = excluded.kind, \
           issuer = excluded.issuer, \
           subject_json = excluded.subject_json, \
           payload = excluded.payload, \
           valid = excluded.valid, \
           ingested_at = excluded.ingested_at",
    )
    .bind(&parsed.id.0)
    .bind(parsed.kind.to_string())
    .bind(parsed.issuer.as_ref().map(|issuer| issuer.0.as_str()))
    .bind(subject_json)
    .bind(&parsed.canonical_payload)
    .execute(database.pool())
    .await
    .map_err(internal_error)?;
    tracing::info!(
        attestation_id = %parsed.id.0,
        kind = %parsed.kind,
        issuer = parsed.issuer.as_ref().map(|issuer| issuer.0.as_str()).unwrap_or(""),
        "installed an attestation"
    );

    Ok(AttestationInstallResponse {
        id: parsed.id,
        kind: parsed.kind,
    })
}

pub(crate) async fn list(database: &Database) -> ServiceResult<AttestationListResponse> {
    let rows = sqlx::query(
        "SELECT id, kind, issuer, subject_json, valid, ingested_at \
         FROM attestation_payloads ORDER BY ingested_at ASC, id ASC",
    )
    .fetch_all(database.pool())
    .await
    .map_err(internal_error)?;

    let payloads = rows
        .into_iter()
        .map(|row| {
            Ok(AttestationPayloadInfo {
                id: AttestationPayloadId(row.get("id")),
                kind: parse_kind(row.get::<String, _>("kind").as_str())?,
                issuer: row.get::<Option<String>, _>("issuer").map(Pubkey),
                subject: serde_json::from_str(row.get::<String, _>("subject_json").as_str())
                    .map_err(internal_error)?,
                ingested_at: Timestamp(i64_to_u64(row.get("ingested_at"))?),
                valid: row.get::<i64, _>("valid") != 0,
            })
        })
        .collect::<ServiceResult<Vec<_>>>()?;

    Ok(AttestationListResponse { payloads })
}

pub(crate) async fn remove(
    database: &Database,
    request: AttestationRemoveRequest,
) -> ServiceResult<AttestationRemoveResponse> {
    let result = match request.target {
        AttestationSelector::Id(id) => sqlx::query("DELETE FROM attestation_payloads WHERE id = ?")
            .bind(&id.0)
            .execute(database.pool())
            .await
            .map_err(internal_error)?,
        AttestationSelector::Issuer(issuer) => {
            sqlx::query("DELETE FROM attestation_payloads WHERE issuer = ?")
                .bind(&issuer.0)
                .execute(database.pool())
                .await
                .map_err(internal_error)?
        }
    };

    if result.rows_affected() == 0 {
        return Err(not_found("attestation payload not found"));
    }

    Ok(AttestationRemoveResponse)
}

pub(crate) async fn summary(database: &Database) -> ServiceResult<AttestationSummary> {
    let rows = sqlx::query(
        "SELECT kind, valid, COUNT(*) AS count FROM attestation_payloads GROUP BY kind, valid",
    )
    .fetch_all(database.pool())
    .await
    .map_err(internal_error)?;
    let mut summary = AttestationSummary::default();
    for row in rows {
        let count = i64_to_u32(row.get("count"))?;
        match parse_kind(row.get::<String, _>("kind").as_str())? {
            AttestationKind::IssuerAuthority => summary.issuer_authorities += count,
        }
        if row.get::<i64, _>("valid") != 0 {
            summary.valid += count;
        } else {
            summary.invalid += count;
        }
    }
    Ok(summary)
}

/// Parse and verify an installed policy document.
///
/// Only an issuer authority installs. A Holder authorization and its backing
/// badge arrive together in the Holder's published event and are enrolled from
/// a relay by `holder_authorization`, so uploading either is refused rather
/// than accepted into a store nothing reads.
fn parse_payload(raw: &[u8]) -> ServiceResult<ParsedPayload> {
    let value: Value = serde_json::from_slice(raw)
        .map_err(|error| invalid_argument(format!("attestation payload is not JSON: {error}")))?;

    let authority = serde_json::from_value::<IssuerAuthority>(value).map_err(|_| {
        invalid_argument("attestation payload is not a recognized issuer authority")
    })?;
    let issuer = authority
        .verify()
        .map_err(|error| invalid_argument(format!("invalid issuer authority: {error}")))?;
    parsed_payload(
        AttestationKind::IssuerAuthority,
        Some(Pubkey(issuer.issuer_id_pubkey.0.to_string())),
        AttestationSubject::Issuer(Pubkey(issuer.issuer_id_pubkey.0.to_string())),
        &authority,
    )
}

fn parsed_payload<T>(
    kind: AttestationKind,
    issuer: Option<Pubkey>,
    subject: AttestationSubject,
    payload: &T,
) -> ServiceResult<ParsedPayload>
where
    T: Serialize,
{
    let canonical = canonical_json_payload(payload).map_err(internal_error)?.0;
    let id_payload = serde_json::json!({
        "kind": kind.to_string(),
        "payload": payload,
    });
    let id_canonical = canonical_json_payload(&id_payload).map_err(internal_error)?;
    let id_hash = domain_tagged_sha256(PAYLOAD_ID_DOMAIN, &id_canonical.0);
    Ok(ParsedPayload {
        id: AttestationPayloadId(hex::encode(id_hash.0)),
        kind,
        issuer,
        subject,
        canonical_payload: canonical,
    })
}

/// Load the installed valid trusted issuer authorities.
pub(crate) async fn trusted_issuer_authorities(
    database: &Database,
) -> ServiceResult<Vec<IssuerAuthority>> {
    let rows = sqlx::query(
        "SELECT payload FROM attestation_payloads WHERE kind = ? AND valid = 1 ORDER BY ingested_at ASC, id ASC",
    )
    .bind(AttestationKind::IssuerAuthority.to_string())
    .fetch_all(database.pool())
    .await
    .map_err(internal_error)?;

    rows.into_iter()
        .map(|row| {
            serde_json::from_slice(&row.get::<Vec<u8>, _>("payload")).map_err(internal_error)
        })
        .collect()
}

#[derive(Debug)]
struct ParsedPayload {
    id: AttestationPayloadId,
    kind: AttestationKind,
    issuer: Option<Pubkey>,
    subject: AttestationSubject,
    canonical_payload: Vec<u8>,
}

fn parse_kind(kind: &str) -> ServiceResult<AttestationKind> {
    kind.parse()
        .map_err(|_| internal_error(format!("unknown attestation kind {kind:?}")))
}

fn i64_to_u64(value: i64) -> ServiceResult<u64> {
    u64::try_from(value).map_err(|_| internal_error(format!("negative timestamp {value}")))
}

fn i64_to_u32(value: i64) -> ServiceResult<u32> {
    u32::try_from(value).map_err(|_| internal_error(format!("count out of range {value}")))
}

#[cfg(test)]
#[path = "../tests/attestation_store.rs"]
mod tests;
