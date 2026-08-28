//! Fleet Manager service protocol.
//!
//! This crate translates the Fedi App ↔ Fleet Manager protocol document into
//! Rust request and response types plus the [`FleetManagerService`] trait.

#![allow(async_fn_in_trait)]

/// The single fedimintd version a 0.1 FMan hosts (and the version an FI
/// requests by default); one hardcoded constant shipping with the release,
/// no runtime probe (SPEC-fi-rpc release policy).
pub const FEDIMINTD_VERSION_0_1: &str = "0.11.1-fedi16";

/// Federation sizes a 0.1 FMan hosts: every size from 7 through 20 inclusive,
/// with 7, 10, and 13 as UI presets and no hidden development-only size
/// (SPEC-fi-rpc release policy).
// review: remove 0.1 references
pub const FEDERATION_SIZES_0_1: [u16; 14] = [7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20];

mod availability;
mod dkg;
mod error;
mod fman_name;
mod locator;
mod locked_payment;
mod maintenance;
mod service;
pub mod signing;
mod status;
mod telemetry;
mod types;

pub use availability::{GetAvailabilityRequest, GetAvailabilityResponse};
pub use dkg::{
    DKG_NAME_MAX_BYTES, DkgCompletionCallback, DkgCompletionCallbackInput, GetDkgCodeRequest,
    GetDkgCodeResponse, InvalidDkgCompletionCallback, InvalidDkgDisplayName, RestartDkgRequest,
    RestartDkgResponse, StartDkgRequest, StartDkgResponse, validate_dkg_display_name,
};
pub use error::FleetManagerError;
pub use fedi_decentralized_services::domain::{
    FMAN_API_URLS_META_FIELD_KEY, FMAN_FEDERATION_TRUST_MATERIAL_SIGNATURE_DOMAIN_SEPARATOR,
    FMAN_TRUST_MATERIAL_MAX_RESPONSE_BYTES, FMAN_TRUST_MATERIAL_PEER_FILTER_MAX_COUNT,
    FederationId, FederationName, FmanApiUrlsMetadata, FmanFederationTrustMaterial,
    FmanFederationTrustMaterialVerificationError, GatewayApiUrl, GetFederationTrustMaterialRequest,
    GetFederationTrustMaterialResponse, InvalidGatewayApiUrl, InviteCode, Timestamp,
};
pub use fedi_decentralized_services::{ServiceError, ServiceErrorCode, ServiceResult};
pub use fman_name::FmanName;
pub use locator::{FLEET_MANAGER_ALPN, Locator, LocatorError};
pub use locked_payment::{
    CreateSeatOutcome, CreateSeatRequest, CreateSeatResponse, GetQuoteRequest, GetQuoteResponse,
    InvalidSeatId, LockedBlindedSignature, LockedIssuanceRequest, LockedIssuanceRequestV2,
    MintGeneration, OfferEpoch, PaymentTerms, QuoteId, QuoteTerms, QuoteTermsError, RefundIssuance,
    RefundTransaction, RefusalReason, SeatId,
};
pub use maintenance::{
    FEDERATION_METADATA_ICON_URL_MAX_BYTES, FEDERATION_METADATA_NAME_MAX_BYTES,
    FEDERATION_METADATA_OBJECT_MAX_BYTES, FEDERATION_METADATA_RAW_MAX_BYTES,
    FEDERATION_METADATA_WELCOME_MESSAGE_MAX_BYTES, FederationMetadataIconUrl,
    FederationMetadataName, FederationMetadataUpdate, FederationMetadataWelcomeMessage,
    GUARDIANITO_TERMS_OF_SERVICE_URL, InvalidFederationMetadataValue,
};
pub use service::{
    FleetManagerService, FleetManagerServiceClient, FleetManagerServiceClientTransport,
    FleetManagerServiceServer, FmResult,
};
pub use signing::{
    AuthError, FI_REQUEST_FRESHNESS_WINDOW_SECS, FI_REQUEST_SIGNATURE_DOMAIN,
    FMAN_RESPONSE_SIGNATURE_DOMAIN, FiSignedRequest, ManagerSignedResponse, SeatScopedFiRequest,
    SignatureVerified, SignedRequest, SignedResponse, VerifiedFiRequest, fi_request_signing_digest,
    manager_response_signing_digest,
};
pub use status::{
    DkgStatusInfo, EndedReason, EndedStatusInfo, FI_GUARDIAN_FEE_WEIGHT, FederationStatusInfo,
    FormationSeatBinding, GUARDIAN_FEE_RECIPIENTS_META_FIELD_KEY,
    GUARDIAN_FEE_SEND_PPM_META_FIELD_KEY, GUARDIAN_GUARDIAN_FEE_WEIGHT,
    GUARDIAN_VERIFICATION_FEE_WEIGHT, GetFedimintStatsRequest, GetFedimintStatsResponse,
    GetInviteCodeRequest, GetInviteCodeResponse, GetPeerAttestationRequest,
    GetPeerAttestationResponse, GetStatusRequest, GetStatusResponse, GuardianFeeAccount,
    GuardianFeeAccountError, GuardianFeeRecipient, GuardianFeeRecipientListError,
    MAX_GUARDIAN_FEE_ACCOUNT_BYTES, MAX_GUARDIAN_FEE_RECIPIENT_LIST_BYTES,
    MAX_GUARDIAN_FEE_RECIPIENTS, PeerConnectionInfo, ProposeFormationMetaRequest,
    ProposeFormationMetaResponse, RegisterGatewayRequest, RegisterGatewayResponse, ServiceStatus,
    SetMetaFieldRequest, SetMetaFieldResponse, StatusDetail, SuspendedStatusInfo,
    canonical_guardian_fee_recipient_list,
};
pub use telemetry::{
    FetchSafeEventJournalRequest, FetchSafeEventJournalResponse, GUARDIAN_TELEMETRY_ALPN,
    GuardianMetricsResponse, GuardianTelemetryApi, GuardianTelemetryApiClient,
    GuardianTelemetryApiServer, GuardianTelemetryRegistrationRequest,
    GuardianTelemetryRegistrationResponse, GuardianTelemetrySeat,
    InvalidSafeEventJournalIncarnation, ListGuardianTelemetrySeatsRequest,
    ListGuardianTelemetrySeatsResponse, ListSafeEventJournalsRequest,
    ListSafeEventJournalsResponse, MAX_GUARDIAN_METRICS_BODY_BYTES,
    MAX_GUARDIAN_TELEMETRY_REGISTRATION_BYTES, MAX_SAFE_EVENT_BATCH_BYTES, SafeEventCursor,
    SafeEventJournal, SafeEventJournalIncarnation, SafeEventJournalInfo,
    ScrapeGuardianMetricsRequest, TelemetryCapability, TelemetryResult,
};
pub use types::{
    AvailabilityInfo, FEDERATION_ICON_URL_META_FIELD_KEY, FEDERATION_NAME_META_FIELD_KEY,
    FederationSize, FedimintStats, FedimintdVersion, FiId, FiSignature, GuardianCode, GuardianName,
    ManagerSignature, MetaConsensusBase, MetaFieldKey, MetaFieldValue, ModuleConfig, OobEcashToken,
    Plan, SeatHealth, TERMS_OF_SERVICE_URL_META_FIELD_KEY, ValidUntilDate,
    WELCOME_MESSAGE_META_FIELD_KEY,
};
