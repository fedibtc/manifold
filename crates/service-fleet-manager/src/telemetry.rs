//! FMan-wide capability-scoped guardian telemetry protocol.
//!
//! This is deliberately a separate Iroh ALPN from the FI-facing Fleet Manager
//! service. One verified FMan registration grants the collector access to seat
//! discovery, guardian metrics, and explicitly-shareable event journals.

use fedi_decentralized_services::domain::{HolderAuthorizationEnvelope, InviteCode, ProtocolV1};
use fedi_iroh_rpc::service;
use serde::{Deserialize, Serialize};

use crate::{SeatId, ServiceResult};

/// Dedicated Iroh ALPN for collector-to-FMan guardian telemetry.
pub const GUARDIAN_TELEMETRY_ALPN: &[u8] = b"fedi/fman/guardian-telemetry/1";

/// Maximum byte length of the upstream Prometheus response body.
pub const MAX_GUARDIAN_METRICS_BODY_BYTES: usize = 4 * 1024 * 1024;

/// Maximum complete JSONL bytes returned by one journal pull.
pub const MAX_SAFE_EVENT_BATCH_BYTES: usize = 768 * 1024;

/// Maximum JSON request body accepted by the Fedi telemetry receiver.
pub const MAX_GUARDIAN_TELEMETRY_REGISTRATION_BYTES: usize = 64 * 1024;

/// High-entropy bearer authorizing all telemetry owned by one FMan.
#[derive(Clone, Eq, PartialEq, Deserialize, Serialize)]
#[serde(transparent)]
pub struct TelemetryCapability([u8; 32]);

impl TelemetryCapability {
    #[must_use]
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    #[must_use]
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl core::fmt::Debug for TelemetryCapability {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str("TelemetryCapability([REDACTED])")
    }
}

/// One FMan registration sent periodically to the governed Fedi receiver.
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GuardianTelemetryRegistrationRequest {
    pub version: ProtocolV1,
    /// Stable Iroh endpoint hosting this FMan's telemetry service.
    pub iroh_endpoint_id: String,
    /// Durable monotone generation of the FMan-wide capability.
    pub generation: u64,
    /// One bearer for seat discovery, metrics, and safe-event journals.
    pub capability: TelemetryCapability,
    /// Current Holder authorization carrying the FMan's PeerBadge.
    pub holder_authorization: HolderAuthorizationEnvelope,
}

impl core::fmt::Debug for GuardianTelemetryRegistrationRequest {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("GuardianTelemetryRegistrationRequest")
            .field("version", &self.version)
            .field("iroh_endpoint_id", &"<redacted>")
            .field("generation", &self.generation)
            .field("capability", &self.capability)
            .field("holder_authorization", &"<redacted>")
            .finish()
    }
}

/// Successful acceptance of one FMan registration.
#[derive(Clone, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GuardianTelemetryRegistrationResponse {
    pub version: ProtocolV1,
}

/// One seat advertised by the authenticated FMan.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GuardianTelemetrySeat {
    pub seat_id: SeatId,
    /// Present after formation; absent for seats that do not yet have an invite.
    pub invite_code: Option<InviteCode>,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ListGuardianTelemetrySeatsRequest {
    pub capability: TelemetryCapability,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ListGuardianTelemetrySeatsResponse {
    pub seats: Vec<GuardianTelemetrySeat>,
}

/// Request one policy-projected scrape of a guardian's loopback Prometheus endpoint.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ScrapeGuardianMetricsRequest {
    /// Resource selection, not an authorization scope.
    pub seat_id: SeatId,
    pub capability: TelemetryCapability,
}

/// Projected guardian metrics response transported over Iroh.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct GuardianMetricsResponse {
    pub status_code: u16,
    pub content_type: Option<String>,
    pub content_encoding: Option<String>,
    #[serde(with = "serde_bytes")]
    pub body: Vec<u8>,
}

/// One independently retained safe-event journal.
#[derive(Clone, Eq, Ord, PartialEq, PartialOrd, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SafeEventJournal {
    Fman,
    Seat { seat_id: SeatId },
}

impl core::fmt::Debug for SafeEventJournal {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str("SafeEventJournal([REDACTED])")
    }
}

/// Durable identity of one safe-event journal storage generation.
///
/// Wire parsing requires canonical lowercase UUIDv7 text. The complete value is
/// identity; its embedded timestamp has no ordering authority.
#[derive(Clone, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct SafeEventJournalIncarnation(String);

impl SafeEventJournalIncarnation {
    /// Return the opaque incarnation text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A journal incarnation was not a canonical lowercase UUIDv7.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct InvalidSafeEventJournalIncarnation;

impl core::fmt::Debug for InvalidSafeEventJournalIncarnation {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str("InvalidSafeEventJournalIncarnation")
    }
}

impl core::fmt::Display for InvalidSafeEventJournalIncarnation {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str("invalid safe-event journal incarnation")
    }
}

impl std::error::Error for InvalidSafeEventJournalIncarnation {}

impl core::str::FromStr for SafeEventJournalIncarnation {
    type Err = InvalidSafeEventJournalIncarnation;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        value.to_owned().try_into()
    }
}

impl TryFrom<String> for SafeEventJournalIncarnation {
    type Error = InvalidSafeEventJournalIncarnation;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        let uuid = uuid::Uuid::parse_str(&value).map_err(|_| InvalidSafeEventJournalIncarnation)?;
        if uuid.get_version() != Some(uuid::Version::SortRand) || uuid.to_string() != value {
            return Err(InvalidSafeEventJournalIncarnation);
        }
        Ok(Self(value))
    }
}

impl<'de> Deserialize<'de> for SafeEventJournalIncarnation {
    fn deserialize<Deserializer>(deserializer: Deserializer) -> Result<Self, Deserializer::Error>
    where
        Deserializer: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        value.parse().map_err(serde::de::Error::custom)
    }
}

impl core::fmt::Debug for SafeEventJournalIncarnation {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str("SafeEventJournalIncarnation([REDACTED])")
    }
}

/// One listed journal and its current storage generation.
#[derive(Clone, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SafeEventJournalInfo {
    pub journal: SafeEventJournal,
    pub incarnation: SafeEventJournalIncarnation,
}

impl core::fmt::Debug for SafeEventJournalInfo {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str("SafeEventJournalInfo([REDACTED])")
    }
}

/// Byte position immediately after a complete stored JSONL record.
#[derive(Clone, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SafeEventCursor {
    /// Storage generation in which the byte coordinates were issued.
    pub incarnation: SafeEventJournalIncarnation,
    /// Durable segment number within this incarnation.
    pub segment: u64,
    /// Byte position immediately after a complete record.
    pub offset: u64,
}

impl core::fmt::Debug for SafeEventCursor {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str("SafeEventCursor([REDACTED])")
    }
}

#[derive(Clone, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ListSafeEventJournalsRequest {
    pub capability: TelemetryCapability,
}

#[derive(Clone, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ListSafeEventJournalsResponse {
    pub journals: Vec<SafeEventJournalInfo>,
}

impl core::fmt::Debug for ListSafeEventJournalsResponse {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("ListSafeEventJournalsResponse")
            .field("journal_count", &self.journals.len())
            .finish()
    }
}

#[derive(Clone, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FetchSafeEventJournalRequest {
    pub capability: TelemetryCapability,
    pub journal: SafeEventJournal,
    /// Journal identity obtained from listing or the previous fetch.
    pub incarnation: SafeEventJournalIncarnation,
    pub cursor: Option<SafeEventCursor>,
}

impl core::fmt::Debug for FetchSafeEventJournalRequest {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str("FetchSafeEventJournalRequest([REDACTED])")
    }
}

#[derive(Clone, Eq, PartialEq, Deserialize, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum FetchSafeEventJournalResponse {
    /// The requested generation is current and the response contains its data.
    Current {
        /// Current durable journal identity.
        incarnation: SafeEventJournalIncarnation,
        /// Complete JSONL records returned by this bounded fetch.
        #[serde(with = "serde_bytes")]
        jsonl: Vec<u8>,
        /// Position after the last returned record.
        next_cursor: Option<SafeEventCursor>,
        /// Retention or invalid coordinates interrupted continuity.
        continuity_gap: bool,
    },
    /// The request or cursor generation is stale and no data was selected.
    IncarnationChanged {
        /// Current durable journal identity.
        incarnation: SafeEventJournalIncarnation,
    },
}

impl core::fmt::Debug for FetchSafeEventJournalResponse {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Current {
                jsonl,
                continuity_gap,
                ..
            } => formatter
                .debug_struct("FetchSafeEventJournalResponse::Current")
                .field("jsonl_bytes", &jsonl.len())
                .field("continuity_gap", continuity_gap)
                .finish(),
            Self::IncarnationChanged { .. } => {
                formatter.write_str("FetchSafeEventJournalResponse::IncarnationChanged")
            }
        }
    }
}

pub type TelemetryResult<T> = ServiceResult<T>;

#[service]
pub trait GuardianTelemetryApi {
    /// List all known seats, including seats without a completed formation.
    async fn list_guardian_telemetry_seats(
        &self,
        request: ListGuardianTelemetrySeatsRequest,
    ) -> TelemetryResult<ListGuardianTelemetrySeatsResponse>;

    async fn scrape_guardian_metrics(
        &self,
        request: ScrapeGuardianMetricsRequest,
    ) -> TelemetryResult<GuardianMetricsResponse>;

    async fn list_safe_event_journals(
        &self,
        request: ListSafeEventJournalsRequest,
    ) -> TelemetryResult<ListSafeEventJournalsResponse>;

    async fn fetch_safe_event_journal(
        &self,
        request: FetchSafeEventJournalRequest,
    ) -> TelemetryResult<FetchSafeEventJournalResponse>;
}

#[cfg(test)]
mod tests {
    use blind_rsa_signatures::Signature as PbrsaSignature;
    use fedi_credential_sdk_protocol::{
        Credential, CredentialDigest, CredentialProof, HolderAuthorization,
        HolderAuthorizationStatement, HolderId, IssuerId, SignedCredential, SubjectPubkey,
        Timestamp as CredentialTimestamp,
    };
    use fedi_decentralized_services::domain::{HolderAuthorizationEnvelope, SchnorrSignatureProof};
    use nostr::{Keys, secp256k1::Message};

    use super::*;
    use crate::QuoteId;

    fn holder_authorization() -> HolderAuthorizationEnvelope {
        let fman = Keys::parse(&format!("{:064x}", 0x7001_u64)).unwrap();
        let holder = Keys::parse(&format!("{:064x}", 0x7002_u64)).unwrap();
        let issuer = Keys::parse(&format!("{:064x}", 0x7003_u64)).unwrap();
        let credential = Credential {
            issuer_id_pubkey: IssuerId(issuer.public_key()),
            info: serde_json::json!({"schema": "fedi-trust-score-v1.0"}),
            blind_msg: serde_json::json!(holder.public_key().to_string()),
        };
        let statement = HolderAuthorizationStatement {
            holder_id_pubkey: HolderId(holder.public_key()),
            subject_pubkey: SubjectPubkey(fman.public_key()),
            credential_digest: CredentialDigest(credential.digest().unwrap()),
            issued_at: CredentialTimestamp(41),
        };
        let signature =
            holder.sign_schnorr(&Message::from_digest(statement.digest().unwrap().into()));
        HolderAuthorizationEnvelope {
            holder_authorization: HolderAuthorization {
                version: ProtocolV1,
                authorization: statement,
                proof: SchnorrSignatureProof { signature },
            },
            signed_credential: SignedCredential {
                version: ProtocolV1,
                credential,
                proof: CredentialProof {
                    signature: PbrsaSignature(vec![1, 2, 3, 4]),
                },
            },
        }
    }

    fn registration_fixture() -> GuardianTelemetryRegistrationRequest {
        GuardianTelemetryRegistrationRequest {
            version: ProtocolV1,
            iroh_endpoint_id: "endpoint-a".to_owned(),
            generation: 17,
            capability: TelemetryCapability::from_bytes([9; 32]),
            holder_authorization: holder_authorization(),
        }
    }

    #[test]
    fn capability_and_registration_debug_are_redacted() {
        let capability = TelemetryCapability::from_bytes([0x42; 32]);
        assert_eq!(format!("{capability:?}"), "TelemetryCapability([REDACTED])");
        let debug = format!("{:?}", registration_fixture());
        assert!(!debug.contains("holder_authorization: Holder"));
        assert!(!debug.contains("endpoint-a"));
        assert!(!debug.contains("[9, 9"));
    }

    #[test]
    fn request_wire_shape_is_stable() {
        let registration = registration_fixture();
        assert_eq!(
            serde_json::to_value(&registration).unwrap(),
            serde_json::json!({
                "version": 1,
                "iroh_endpoint_id": "endpoint-a",
                "generation": 17,
                "capability": vec![9; 32],
                "holder_authorization": serde_json::to_value(
                    &registration.holder_authorization
                ).unwrap(),
            })
        );
        let mut legacy = serde_json::to_value(&registration).unwrap();
        legacy.as_object_mut().unwrap().remove("generation");
        assert!(
            serde_json::from_value::<GuardianTelemetryRegistrationRequest>(legacy).is_err(),
            "the required generation deliberately rejects the old wire shape"
        );

        let request = ListGuardianTelemetrySeatsRequest {
            capability: TelemetryCapability::from_bytes([8; 32]),
        };
        assert_eq!(
            serde_json::to_value(request).unwrap(),
            serde_json::json!({"capability": vec![8; 32]})
        );

        let request = FetchSafeEventJournalRequest {
            capability: TelemetryCapability::from_bytes([8; 32]),
            journal: SafeEventJournal::Seat {
                seat_id: SeatId::from(QuoteId([7; 32])),
            },
            incarnation: "018f22d0-4e5f-7abc-8def-0123456789ab".parse().unwrap(),
            cursor: Some(SafeEventCursor {
                incarnation: "018f22d0-4e5f-7abc-8def-0123456789ab".parse().unwrap(),
                segment: 42,
                offset: 512,
            }),
        };
        assert_eq!(
            serde_json::to_value(request).unwrap(),
            serde_json::json!({
                "capability": vec![8; 32],
                "journal": {
                    "kind": "seat",
                    "seat_id": hex::encode([7; 32]),
                },
                "incarnation": "018f22d0-4e5f-7abc-8def-0123456789ab",
                "cursor": {
                    "incarnation": "018f22d0-4e5f-7abc-8def-0123456789ab",
                    "segment": 42,
                    "offset": 512
                },
            })
        );
    }

    #[test]
    fn maximum_safe_event_batch_has_bounded_cbor_encoding() {
        let response = FetchSafeEventJournalResponse::Current {
            incarnation: "018f22d0-4e5f-7abc-8def-0123456789ab".parse().unwrap(),
            jsonl: vec![0x7f; MAX_SAFE_EVENT_BATCH_BYTES],
            next_cursor: Some(SafeEventCursor {
                incarnation: "018f22d0-4e5f-7abc-8def-0123456789ab".parse().unwrap(),
                segment: 42,
                offset: MAX_SAFE_EVENT_BATCH_BYTES as u64,
            }),
            continuity_gap: false,
        };
        let mut encoded = Vec::new();
        ciborium::into_writer(&response, &mut encoded).unwrap();
        assert!(encoded.len() < 769 * 1024);
    }

    #[test]
    fn incarnation_validation_and_fetch_wire_variants_are_exact() {
        let text = "018f22d0-4e5f-7abc-8def-0123456789ab";
        let incarnation: SafeEventJournalIncarnation = text.parse().unwrap();
        for invalid in [
            "not-a-uuid",
            "00000000-0000-0000-0000-000000000000",
            "018F22D0-4E5F-7ABC-8DEF-0123456789AB",
        ] {
            assert!(invalid.parse::<SafeEventJournalIncarnation>().is_err());
            assert!(
                serde_json::from_value::<SafeEventJournalIncarnation>(serde_json::Value::String(
                    invalid.to_owned()
                ))
                .is_err()
            );
        }

        let list = ListSafeEventJournalsResponse {
            journals: vec![SafeEventJournalInfo {
                journal: SafeEventJournal::Fman,
                incarnation: incarnation.clone(),
            }],
        };
        assert_eq!(
            serde_json::to_value(list).unwrap(),
            serde_json::json!({
                "journals": [{
                    "journal": {"kind": "fman"},
                    "incarnation": text,
                }]
            })
        );

        let cursor = SafeEventCursor {
            incarnation: incarnation.clone(),
            segment: 42,
            offset: 512,
        };
        let current = FetchSafeEventJournalResponse::Current {
            incarnation: incarnation.clone(),
            jsonl: b"{\"safe\":true}\n".to_vec(),
            next_cursor: Some(cursor),
            continuity_gap: false,
        };
        assert_eq!(
            serde_json::to_value(current).unwrap(),
            serde_json::json!({
                "status": "current",
                "incarnation": text,
                "jsonl": b"{\"safe\":true}\n".to_vec(),
                "next_cursor": {
                    "incarnation": text,
                    "segment": 42,
                    "offset": 512,
                },
                "continuity_gap": false,
            })
        );
        assert_eq!(
            serde_json::to_value(FetchSafeEventJournalResponse::IncarnationChanged { incarnation })
                .unwrap(),
            serde_json::json!({
                "status": "incarnation_changed",
                "incarnation": text,
            })
        );
    }

    #[test]
    fn safe_event_protocol_debug_redacts_selectors_coordinates_and_body() {
        let incarnation: SafeEventJournalIncarnation =
            "018f22d0-4e5f-7abc-8def-0123456789ab".parse().unwrap();
        let seat_id = SeatId::from(QuoteId([0x7a; 32]));
        let cursor = SafeEventCursor {
            incarnation: incarnation.clone(),
            segment: 987_654,
            offset: 123_456,
        };
        let request = FetchSafeEventJournalRequest {
            capability: TelemetryCapability::from_bytes([8; 32]),
            journal: SafeEventJournal::Seat {
                seat_id: seat_id.clone(),
            },
            incarnation: incarnation.clone(),
            cursor: Some(cursor.clone()),
        };
        let list = ListSafeEventJournalsResponse {
            journals: vec![SafeEventJournalInfo {
                journal: SafeEventJournal::Seat { seat_id },
                incarnation: incarnation.clone(),
            }],
        };
        let current = FetchSafeEventJournalResponse::Current {
            incarnation: incarnation.clone(),
            jsonl: b"SECRET_BODY_SENTINEL\n".to_vec(),
            next_cursor: Some(cursor),
            continuity_gap: false,
        };
        let changed = FetchSafeEventJournalResponse::IncarnationChanged { incarnation };

        for debug in [
            format!("{request:?}"),
            format!("{list:?}"),
            format!("{current:?}"),
            format!("{changed:?}"),
        ] {
            assert!(!debug.contains("018f22d0"));
            assert!(!debug.contains("987654"));
            assert!(!debug.contains("123456"));
            assert!(!debug.contains("7a7a7a"));
            assert!(!debug.contains("SECRET_BODY_SENTINEL"));
        }
    }
}
