//! Manifold Guardian service protocol.
//!
//! This crate translates the Fedi App ↔ Manifold Guardian protocol document into
//! Rust request and response types plus the [`ManifoldGuardianService`] trait.

#![allow(async_fn_in_trait)]

/// The single fedimintd version a 0.1 MG hosts (and the version an FA
/// requests by default); one hardcoded constant shipping with the release,
/// no runtime probe (SPEC-fa-rpc release policy).
pub const FEDIMINTD_VERSION_0_1: &str = "0.12.0+fedi";

/// Federation sizes a 0.1 MG hosts: every size from 7 through 20 inclusive,
/// with 7, 10, and 13 as UI presets and no hidden development-only size
/// (SPEC-fa-rpc release policy).
// review: remove 0.1 references
pub const FEDERATION_SIZES_0_1: [u16; 14] = [7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20];

mod availability;
mod decommission;
mod dkg;
mod error;
mod locator;
mod locked_payment;
mod maintenance;
mod mg_name;
mod service;
pub mod signing;
mod status;
mod telemetry;
mod transfer;
mod types;

pub use availability::{GetAvailabilityRequest, GetAvailabilityResponse};
pub use decommission::{DecommissionSeatRequest, DecommissionSeatResponse};
pub use dkg::{
    DKG_NAME_MAX_BYTES, DkgCompletionCallback, DkgCompletionCallbackInput, GetDkgCodeRequest,
    GetDkgCodeResponse, InvalidDkgCompletionCallback, InvalidDkgDisplayName, RestartDkgRequest,
    RestartDkgResponse, StartDkgRequest, StartDkgResponse, validate_dkg_display_name,
};
pub use error::ManifoldGuardianError;
pub use fedi_decentralized_services::domain::{
    FederationId, FederationName, GatewayApiUrl, GetMgTrustMaterialRequest,
    GetMgTrustMaterialResponse, InvalidGatewayApiUrl, InviteCode, MG_API_URLS_META_FIELD_KEY,
    MG_TRUST_MATERIAL_MAX_RESPONSE_BYTES, MG_TRUST_MATERIAL_SIGNATURE_DOMAIN_SEPARATOR,
    MgApiUrlsMetadata, MgTrustMaterial, MgTrustMaterialVerificationError, Timestamp,
};
pub use fedi_decentralized_services::{ServiceError, ServiceErrorCode, ServiceResult};
pub use locator::{
    InvalidEndpointAddrPair, Locator, MANIFOLD_GUARDIAN_ALPN, MG_E2E_ENDPOINT_ADDRS_ENV,
    parse_endpoint_addr_pairs,
};
pub use locked_payment::{
    CreateSeatOutcome, CreateSeatRequest, CreateSeatResponse, GetQuoteRequest, GetQuoteResponse,
    InvalidSeatId, LockedBlindedSignature, LockedIssuanceRequest, LockedIssuanceRequestV2,
    MintGeneration, OfferEpoch, PaymentTerms, QuoteId, QuoteTerms, QuoteTermsError, RefundIssuance,
    RefundTransaction, RefusalReason, SeatId,
};
pub use maintenance::{
    FEDERATION_APP_SETTING_MAX_COMMUNITIES, FEDERATION_APP_SETTING_MAX_GROUP_CHATS,
    FEDERATION_APP_SETTING_MAX_MSATS, FEDERATION_METADATA_ICON_URL_MAX_BYTES,
    FEDERATION_METADATA_NAME_MAX_BYTES, FEDERATION_METADATA_OBJECT_MAX_BYTES,
    FEDERATION_METADATA_RAW_MAX_BYTES, FEDERATION_METADATA_TERMS_URL_MAX_BYTES,
    FEDERATION_METADATA_WELCOME_MESSAGE_MAX_BYTES, FederationAppSetting, FederationAppSettingValue,
    FederationMetadataIconUrl, FederationMetadataName, FederationMetadataTermsUrl,
    FederationMetadataUpdate, FederationMetadataWelcomeMessage, InvalidFederationMetadataValue,
};
pub use mg_name::MgName;
pub use service::{
    FmResult, ManifoldGuardianService, ManifoldGuardianServiceClient,
    ManifoldGuardianServiceClientTransport, ManifoldGuardianServiceServer,
};
pub use signing::{
    AuthError, FA_REQUEST_FRESHNESS_WINDOW_SECS, FA_REQUEST_SIGNATURE_DOMAIN, FaSignedRequest,
    MG_REQUEST_SIGNATURE_DOMAIN, MG_RESPONSE_SIGNATURE_DOMAIN, ManagerSignedRequest,
    ManagerSignedResponse, MgSignedRequest, SeatScopedFaRequest, SignatureVerified, SignedRequest,
    SignedResponse, VerifiedFaRequest, fa_request_signing_digest, manager_request_signing_digest,
    manager_response_signing_digest,
};
pub use status::{
    DkgStatusInfo, EndedReason, EndedStatusInfo, FA_GUARDIAN_FEE_WEIGHT, FederationStatusInfo,
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
    MAX_GUARDIAN_TELEMETRY_REGISTRATION_BYTES, MAX_SAFE_EVENT_BATCH_BYTES, SEAT_CAPACITY_EVENT,
    SEAT_READINESS_EVENT, SafeEventCursor, SafeEventJournal, SafeEventJournalIncarnation,
    SafeEventJournalInfo, ScrapeGuardianMetricsRequest, SeatCapacityEvent, SeatReadinessEvent,
    TelemetryCapability, TelemetryResult,
};
pub use transfer::{
    GuardianMaterial, GuardianMaterialTooLarge, PullSeatRequest, PullSeatResponse, RebindSeatError,
    ReboundSeat, RemoveSeatRequest, RemoveSeatResponse, ReplaceSeatBindingRequest,
    ReplaceSeatBindingResponse, SEAT_TRANSFER_MATERIAL_MAX_BYTES, SeatTransferGrant,
    TransferSeatRequest, TransferSeatResponse, rebind_transferred_seat,
};
pub use types::{
    AvailabilityInfo, FEDERATION_ICON_URL_META_FIELD_KEY, FEDERATION_NAME_META_FIELD_KEY, FaId,
    FaSignature, FederationSize, FedimintStats, FedimintdDkgVersion, FedimintdVersion,
    FedimintdVersionCore, GuardianCode, GuardianName, ManagerSignature, MetaConsensusBase,
    MetaFieldKey, MetaFieldValue, ModuleConfig, OobEcashToken, Plan, SeatHealth,
    TERMS_OF_SERVICE_URL_META_FIELD_KEY, ValidUntilDate, WELCOME_MESSAGE_META_FIELD_KEY,
};
