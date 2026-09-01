//! L6 RPC service: the thin boundary between the wire and the fleet.
//!
//! Everything here is mapping: verify the [`SignedRequest`] envelope into a
//! `VerifiedFiRequest<T>` (the only unauthenticated code path is this one line),
//! check the 0.1 policy gates, call the L4 [`Fleet`] verb, and translate its
//! typed error into the wire [`FleetManagerError`] vocabulary. Internal
//! failures are logged with detail and answered with the generic
//! `Other("internal error")`; envelope failures are logged with their
//! [`AuthError`](fedi_decentralized_service_fleet_manager::AuthError) detail
//! and answered with the deliberately coarse `Unauthorized`.

use std::sync::{Arc, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use fedi_decentralized_domain::{
    AdmittedSetupPaymentFederations, DEFAULT_SETUP_PAYMENT_MIN_FEE_PPM,
    FmanFederationTrustMaterial, FmanPeerAttestation, FmanPeerAttestationStatement,
    HolderAuthorizationEnvelope, ProtocolV1, Pubkey, SchnorrSignatureProof, Url,
};
use fedi_decentralized_service_fleet_manager::{
    CreateSeatOutcome, CreateSeatRequest, CreateSeatResponse, DkgStatusInfo, EndedReason,
    EndedStatusInfo, FEDERATION_SIZES_0_1, FEDIMINTD_VERSION_0_1, FederationSize,
    FederationStatusInfo, FedimintdVersion, FiSignedRequest, FleetManagerError,
    FleetManagerService, FmResult, GetAvailabilityRequest, GetAvailabilityResponse,
    GetDkgCodeRequest, GetDkgCodeResponse, GetFederationTrustMaterialRequest,
    GetFederationTrustMaterialResponse, GetFedimintStatsRequest, GetFedimintStatsResponse,
    GetInviteCodeRequest, GetInviteCodeResponse, GetPeerAttestationRequest,
    GetPeerAttestationResponse, GetStatusRequest, GetStatusResponse, GuardianFeeAccount, Plan,
    ProposeFormationMetaRequest, ProposeFormationMetaResponse, QuoteTerms, RegisterGatewayRequest,
    RegisterGatewayResponse, RestartDkgRequest, RestartDkgResponse, ServiceStatus,
    SetMetaFieldRequest, SetMetaFieldResponse, SignedRequest, SignedResponse, StartDkgRequest,
    StartDkgResponse, StatusDetail, Timestamp, VerifiedFiRequest,
};
use fedimint_core::util::SafeUrl;
use secp256k1::Keypair;
use stability_pool_client::common::Account;

use crate::fleet::{Fleet, VerifiedCreateSeat, VerifiedPayment};
use crate::push_callback::ValidatedDkgCompletionCallback;
use crate::seat::{SeatFederationBinding, SeatPhase, SeatReport, SeatVerbError};
use crate::wallet::{LockedPaymentPrepareError, Msats};

/// The Fleet Manager 0.1 RPC service: one [`Fleet`] plus the service signing
/// key whose public half is advertised as the locator `service_pubkey`.
///
/// Seat access discipline: every verb naming an existing seat resolves it
/// through `Fleet::authorize` — the fleet's only crate-visible seat getter —
/// as its first fleet call, and no verb here calls an operator verb
/// (decommission, listing, shutdown). This file is the entire FI surface;
/// keep it scannable for those two facts.
#[derive(Clone)]
pub struct FleetManagerRpc {
    fleet: Arc<Fleet>,
    signing_key: Keypair,
    /// Deployment-pinned Guardian Verification Fee account.
    /// The fee-proposal path fails closed if this is absent.
    guardian_verification_fee_account: Option<Account>,
    /// The FMan's public service identity, which signs peer attestations.
    ///
    /// Deliberately the *same* key `fman-nostr` signs kind-37701
    /// with, not the
    /// commitment `signing_key` beside it: a verifier resolves an FMan's live
    /// trust by looking its advertisement up *by* this pubkey, so an
    /// attestation naming any other key resolves to nothing
    /// ([`ARCH-fleet-manager-identity`](../../specs/ARCH-fleet-manager-identity.md)).
    attestation_keys: nostr_sdk::Keys,

    /// Inputs `get_federation_trust_material` needs that do not exist yet when
    /// this service is constructed.
    ///
    /// The daemon's wiring is genuinely circular: this service goes into the
    /// Iroh router, the router owns the endpoint whose id the Nostr runtime
    /// advertises, and the Nostr runtime is where holder authorizations come
    /// from. Rather than unpick that ordering, the daemon binds the source once
    /// both exist. Unbound is an explicit fail-closed construction state:
    /// a request arriving before the runtime source is bound cannot serve trust
    /// material from absent or partial inputs.
    trust_material: Arc<OnceLock<Arc<dyn TrustMaterialSource>>>,

    /// The Fedi-published setup-payment policy as the Nostr boundary last
    /// admitted it, which is where the minimum guardian fee rate comes from
    /// ([SPEC-setup-payment-federations](../../../../specs/SPEC-setup-payment-federations.md)).
    ///
    /// The Nostr boundary exists before the RPC router and supplies this
    /// receiver at construction. An admitted event not yet seen falls back to
    /// [`DEFAULT_SETUP_PAYMENT_MIN_FEE_PPM`] rather than to zero — a missing
    /// publication must not read as "any rate is acceptable".
    setup_payment_policy: tokio::sync::watch::Receiver<Option<AdmittedSetupPaymentFederations>>,
}

/// The late-bound inputs behind `get_federation_trust_material`.
///
/// A port rather than a direct `fman-nostr` handle so the RPC layer
/// keeps depending only on what it uses, and so tests can serve trust material
/// without standing up a relay.
pub trait TrustMaterialSource: Send + Sync {
    /// This FMan's dialable Iroh endpoint, as an `iroh://` URL.
    fn iroh_endpoint_url(&self) -> Url;

    /// The verified holder authorizations durably enrolled by the operator.
    fn holder_authorizations(&self) -> Vec<HolderAuthorizationEnvelope>;
}

impl FleetManagerRpc {
    /// Build the service with its deployment policy inputs.
    pub fn new(
        fleet: Arc<Fleet>,
        guardian_verification_fee_account: Option<Account>,
        setup_payment_policy: tokio::sync::watch::Receiver<Option<AdmittedSetupPaymentFederations>>,
    ) -> Self {
        let signing_key = fleet.identity().derive_service_signing_key();
        let attestation_keys = nostr_sdk::Keys::new(
            nostr_sdk::SecretKey::from_slice(
                &fleet
                    .identity()
                    .derive_service_nostr_secret_key()
                    .secret_bytes(),
            )
            .expect("HKDF-derived nostr key is valid"),
        );
        Self {
            fleet,
            signing_key,
            guardian_verification_fee_account,
            attestation_keys,
            trust_material: Arc::new(OnceLock::new()),
            setup_payment_policy,
        }
    }

    /// The minimum guardian fee rate an FI may propose right now, in ppm.
    ///
    /// With no admitted event this is the published default, never zero.
    fn min_guardian_fee_ppm(&self) -> u64 {
        self.setup_payment_policy
            .borrow()
            .as_ref()
            .map(AdmittedSetupPaymentFederations::min_fee_ppm)
            .unwrap_or(DEFAULT_SETUP_PAYMENT_MIN_FEE_PPM)
    }

    /// Supply the trust-material inputs once the endpoint and Nostr runtime
    /// exist. Ignores a second call rather than panicking: binding twice is a
    /// wiring mistake, not a reason to take the daemon down after it is
    /// already serving.
    pub fn bind_trust_material_source(&self, source: Arc<dyn TrustMaterialSource>) {
        if self.trust_material.set(source).is_err() {
            tracing::warn!(
                safe_to_share = true,
                "trust-material source was already bound; ignoring the second binding"
            );
        }
    }

    /// Verify a signed envelope; failures are logged and answered
    /// `Unauthorized` without detail.
    fn validate<T: FiSignedRequest>(
        &self,
        request: SignedRequest<T>,
    ) -> FmResult<VerifiedFiRequest<T>> {
        request.verify(now()).map_err(|err| {
            tracing::warn!(verb = T::LABEL, error = %err, "rejecting unauthorized request");
            FleetManagerError::Unauthorized
        })
    }

    fn validate_completion_callback(
        &self,
        callback: &fedi_decentralized_service_fleet_manager::DkgCompletionCallback,
    ) -> FmResult<ValidatedDkgCompletionCallback> {
        let origin = self
            .fleet
            .config()
            .push_gateway_origin
            .as_ref()
            .ok_or_else(|| {
                FleetManagerError::InvalidDkgInput(
                    "DKG completion callbacks are not configured on this Fleet Manager".to_owned(),
                )
            })?;
        origin.validate(callback).map_err(|error| {
            FleetManagerError::InvalidDkgInput(format!("invalid DKG completion callback: {error}"))
        })
    }
}

/// How long a signed trust-material response stays valid.
///
/// Deliberately short. The relying verifier applies its own upper bound on
/// `expires_at - issued_at`, and this is the FMan's side of that bargain: the
/// FI collects material for a request it is about to make, so a long window
/// buys nothing and only widens how far a withdrawn FMan's material outlives
/// it. One hour leaves room for a slow formation without becoming a standing
/// credential.
const TRUST_MATERIAL_VALIDITY_SECS: u64 = 3600;

fn now() -> Timestamp {
    Timestamp(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock is after Unix epoch")
            .as_secs(),
    )
}

fn internal(verb: &'static str, err: impl std::fmt::Display) -> FleetManagerError {
    tracing::error!(verb, error = %err, "internal error");
    FleetManagerError::Other("internal error".to_owned())
}

/// This log line is the only place an internal failure's detail surfaces
/// (the wire deliberately answers a bare "internal error"), so it must
/// carry the whole cause chain — anyhow's `Display` alone prints only the
/// top message.
fn internal_chain(verb: &'static str, err: &anyhow::Error) -> FleetManagerError {
    internal(verb, format_args!("{err:#}"))
}

fn map_seat_error(verb: &'static str, err: SeatVerbError) -> FleetManagerError {
    match err {
        SeatVerbError::UnknownSeat => FleetManagerError::UnknownSeat,
        SeatVerbError::SeatUnavailable => FleetManagerError::SeatUnavailable,
        SeatVerbError::FederationIsRunning => FleetManagerError::FederationIsRunning,
        SeatVerbError::WrongState { status } => FleetManagerError::WrongState { status },
        SeatVerbError::InvalidDkgInput(reason) => FleetManagerError::InvalidDkgInput(reason),
        // Both stay coarse on the wire: the refusal reason is daemon log
        // material, and the compiled key set is not worth publishing to an
        // unauthenticated prober.
        SeatVerbError::MetaKeyRefused => FleetManagerError::MetaKeyRefused,
        SeatVerbError::MetaValueInvalid => FleetManagerError::MetaValueInvalid,
        SeatVerbError::MetaConsensusChanged => FleetManagerError::MetaConsensusChanged,
        SeatVerbError::FormationMetaAlreadyPublished => {
            FleetManagerError::FormationMetaAlreadyPublished
        }
        SeatVerbError::MetaTargetConflict => FleetManagerError::MetaTargetConflict,
        SeatVerbError::Internal(err) => internal_chain(verb, &err),
    }
}

fn fleet_manager_error_kind(error: &FleetManagerError) -> &'static str {
    match error {
        FleetManagerError::PlanNotOffered => "plan_not_offered",
        FleetManagerError::PaymentFederationNotAccepted => "payment_federation_not_accepted",
        FleetManagerError::InvalidPayment => "invalid_payment",
        FleetManagerError::CapacityExhausted => "capacity_exhausted",
        FleetManagerError::UnknownSeat => "unknown_seat",
        FleetManagerError::UnsupportedVersion => "unsupported_version",
        FleetManagerError::UnsupportedFederationSize => "unsupported_federation_size",
        FleetManagerError::Unauthorized => "unauthorized",
        FleetManagerError::WrongState { .. } => "wrong_state",
        FleetManagerError::SeatUnavailable => "seat_unavailable",
        FleetManagerError::FederationIsRunning => "federation_is_running",
        FleetManagerError::InvalidDkgInput(_) => "invalid_dkg_input",
        FleetManagerError::MetaKeyRefused => "meta_key_refused",
        FleetManagerError::MetaValueInvalid => "meta_value_invalid",
        FleetManagerError::GuardianVerificationFeeAccountUnavailable => {
            "guardian_verification_fee_account_unavailable"
        }
        FleetManagerError::GuardianVerificationFeeAccountMismatch => {
            "guardian_verification_fee_account_mismatch"
        }
        FleetManagerError::MetaConsensusChanged => "meta_consensus_changed",
        FleetManagerError::FormationMetaAlreadyPublished => "formation_meta_already_published",
        FleetManagerError::MetaTargetConflict => "meta_target_conflict",
        FleetManagerError::InvalidGatewayApiUrl => "invalid_gateway_api_url",
        FleetManagerError::UnknownFederation => "unknown_federation",
        FleetManagerError::FederationConfigHashMismatch => "federation_config_hash_mismatch",
        FleetManagerError::InvalidTrustMaterialSelector => "invalid_trust_material_selector",
        FleetManagerError::TrustMaterialUnavailable => "trust_material_unavailable",
        FleetManagerError::UnsupportedVerb { .. } => "unsupported_verb",
        FleetManagerError::Other(_) => "internal_or_other",
    }
}

async fn trace_formation_rpc<T>(
    verb: &'static str,
    future: impl std::future::Future<Output = FmResult<T>>,
) -> FmResult<T> {
    let result = future.await;
    if let Err(error) = &result {
        tracing::warn!(verb, %error, "formation RPC failed");
        tracing::warn!(
            safe_to_share = true,
            verb,
            failure_kind = fleet_manager_error_kind(error),
            "formation RPC failed"
        );
    }
    result
}

/// SPEC-fi-rpc's display-name rule, enforced at the boundary and answered as
/// the typed policy error. The wire wrappers stay permissive so a malformed
/// authenticated request reaches this check instead of failing decoding.
fn validate_dkg_name(kind: &str, value: &str) -> FmResult<()> {
    fedi_decentralized_service_fleet_manager::validate_dkg_display_name(value)
        .map_err(|err| FleetManagerError::InvalidDkgInput(format!("{kind} {err}")))
}

fn unsupported<T>(verb: &str) -> FmResult<T> {
    Err(FleetManagerError::UnsupportedVerb {
        verb: verb.to_owned(),
    })
}

/// Sign this seat's binding as an [`FmanPeerAttestation`].
///
/// Every field but `issued_at` comes from the seat's own live config through
/// [`SeatFederationBinding`], and the digest is the shared statement's — the
/// FMan derives nothing here that a verifier re-derives independently, which
/// is what keeps the two from disagreeing about the same federation
/// ([`SPEC-federation-trust-directory`](../../../domain/specs/SPEC-federation-trust-directory.md)).
pub(crate) fn sign_peer_attestation(
    keys: &nostr_sdk::Keys,
    binding: &SeatFederationBinding,
    guardian_fee_account: Account,
    issued_at: Timestamp,
) -> Result<FmanPeerAttestation, serde_json::Error> {
    let attestation = FmanPeerAttestationStatement {
        fman_pubkey: Pubkey(keys.public_key().to_string()),
        federation_id: binding.federation.federation_id().clone(),
        federation_config_hash: binding.federation.federation_config_hash().clone(),
        peer_id: binding.seat.peer_id.clone(),
        guardian_identity: binding.seat.guardian_identity.clone(),
        guardian_fee_account,
        issued_at,
    };
    let signature = keys.sign_schnorr(&nostr_sdk::secp256k1::Message::from_digest(
        attestation.digest()?,
    ));

    Ok(FmanPeerAttestation {
        version: ProtocolV1,
        attestation,
        proof: SchnorrSignatureProof { signature },
    })
}

fn supported_fedimintd_version() -> FedimintdVersion {
    FEDIMINTD_VERSION_0_1
        .parse()
        .expect("FEDIMINTD_VERSION_0_1 must be valid SemVer")
}

impl FleetManagerService for FleetManagerRpc {
    async fn get_availability(
        &self,
        _request: GetAvailabilityRequest,
    ) -> FmResult<GetAvailabilityResponse> {
        let availability = self.fleet.availability_snapshot().await;
        let mut federation_sizes: Vec<_> = FEDERATION_SIZES_0_1
            .into_iter()
            .map(FederationSize)
            .collect();
        if is_local_single_guardian_e2e(1) {
            federation_sizes.push(FederationSize(1));
        }
        Ok(GetAvailabilityResponse {
            // Use the same wallet-gated projection as advertising. Independent
            // calls may observe different settings epochs or live state.
            accepting_seats: availability.accepting_seats,
            fedimintd_version: supported_fedimintd_version(),
            federation_sizes,
            plans: availability.plans,
            additional_info: vec![],
        })
    }

    async fn get_quote(
        &self,
        request: fedi_decentralized_service_fleet_manager::GetQuoteRequest,
    ) -> FmResult<SignedResponse<fedi_decentralized_service_fleet_manager::GetQuoteResponse>> {
        trace_formation_rpc("get_quote", async {
            if request.fedimintd_version != supported_fedimintd_version() {
                return Err(FleetManagerError::UnsupportedVersion);
            }
            if !FEDERATION_SIZES_0_1.contains(&request.federation_size.0)
                && !is_local_single_guardian_e2e(request.federation_size.0)
            {
                return Err(FleetManagerError::UnsupportedFederationSize);
            }
            let offer = self
                .fleet
                .quote_offer()
                .await
                .ok_or(FleetManagerError::CapacityExhausted)?;
            let offer_epoch = offer.epoch;
            let settings = offer.settings;
            if !settings.plans().contains(&request.plan) {
                return Err(FleetManagerError::PlanNotOffered);
            }
            let price = match &request.plan {
                Plan::InfiniteBestEffort { price_msats } => *price_msats,
                Plan::SubscriptionBased { .. } => {
                    // The offer is stored as a price and rendered as the one
                    // plan this daemon serves, so no stored offer can contain
                    // this variant; quoting one would sell a subscription v1
                    // has no renewal machinery for.
                    return Err(internal(
                        "get_quote",
                        anyhow::anyhow!("SubscriptionBased plan in the stored offer"),
                    ));
                }
            };
            let quote_nonce: [u8; 32] = rand::random();
            let payment = if price == 0 {
                // A seat given away settles against nothing, so terms that name a
                // payment federation or refund outputs are not expressible for it.
                // Refuse rather than quote something the requester did not ask for.
                if request.payment_federation_id.is_some() || request.refund_issuance.is_some() {
                    return Err(FleetManagerError::PlanNotOffered);
                }
                None
            } else {
                let selected = request
                    .payment_federation_id
                    .as_ref()
                    .ok_or(FleetManagerError::PaymentFederationNotAccepted)?;
                if !settings.payment_federations.contains(selected) {
                    return Err(FleetManagerError::PaymentFederationNotAccepted);
                }
                let quoted = self
                    .fleet
                    .wallet()
                    .quote_locked(selected, Msats(price), &quote_nonce)
                    .await
                    .map_err(|err| match err {
                        LockedPaymentPrepareError::Invalid => {
                            FleetManagerError::PaymentFederationNotAccepted
                        }
                        LockedPaymentPrepareError::Internal(err) => {
                            internal_chain("get_quote", &err)
                        }
                    })?;
                let refund = request
                    .refund_issuance
                    .as_ref()
                    .ok_or(FleetManagerError::PlanNotOffered)?;
                self.fleet
                    .wallet()
                    .validate_quote_refund(&quoted, refund)
                    .await
                    .map_err(|err| match err {
                        LockedPaymentPrepareError::Invalid => FleetManagerError::PlanNotOffered,
                        LockedPaymentPrepareError::Internal(err) => {
                            internal_chain("get_quote", &err)
                        }
                    })?;
                Some(quoted)
            };
            let terms = QuoteTerms::compose(request, offer_epoch, price, quote_nonce, payment)
                .map_err(|err| internal("get_quote", err))?;
            let response = fedi_decentralized_service_fleet_manager::GetQuoteResponse { terms };
            SignedResponse::create(&response, &self.signing_key)
                .map_err(|err| internal("get_quote", err))
        })
        .await
    }

    async fn create_seat(
        &self,
        request: SignedRequest<CreateSeatRequest>,
    ) -> FmResult<SignedResponse<CreateSeatResponse>> {
        trace_formation_rpc("create_seat", async {
            let request = self.validate(request)?;
            let signer = *request.signer();
            let request = request.into_inner();
            // Only a manager-signed quote can name an allocation decision; its
            // embedded FI must be the verified request signer.
            let quote = request
                .quote
                .verify(&self.signing_key.x_only_public_key().0)
                .map_err(|_| FleetManagerError::InvalidPayment)?;
            quote
                .terms
                .check_coherent()
                .map_err(|_| FleetManagerError::InvalidPayment)?;
            let quote_id = quote.quote_id();
            if quote.terms.request.fi_id != signer {
                return Err(FleetManagerError::InvalidPayment);
            }
            let payment = match &quote.terms.payment {
                None => {
                    // A free quote priced nothing, so signatures presented against
                    // it can only be confusion or an attempt to have them read as
                    // evidence of something.
                    if !request.payment_signatures.is_empty() {
                        return Err(FleetManagerError::InvalidPayment);
                    }
                    VerifiedPayment::Free
                }
                Some(terms) => {
                    // Payment verification remains outside allocation; the fleet
                    // receives only the opaque verified payment capability.
                    let verified = self
                        .fleet
                        .wallet()
                        .verify_locked(&quote_id, &quote.terms, &request.payment_signatures)
                        .await
                        .map_err(|err| match err {
                            LockedPaymentPrepareError::Invalid => FleetManagerError::InvalidPayment,
                            LockedPaymentPrepareError::Internal(err) => {
                                internal_chain("create_seat", &err)
                            }
                        })?;
                    VerifiedPayment::Locked {
                        federation_id: terms.federation_id().clone(),
                        payment: verified,
                    }
                }
            };

            let quote = quote.into_inner();
            self.fleet
                .create_seat(
                    VerifiedCreateSeat {
                        fi_id: signer,
                        quote_id,
                        quote_terms: quote.terms,
                        payment,
                    },
                    |seat_id| {
                        let guardian_fee_account = GuardianFeeAccount::try_from(
                            self.fleet.guardian_fee_account_descriptor(seat_id),
                        )
                        .expect("mnemonic-derived guardian fee account is single-sig BtcDepositor");
                        let response = CreateSeatResponse {
                            quote_id,
                            outcome: CreateSeatOutcome::Accepted {
                                seat_id: seat_id.clone(),
                                guardian_fee_account,
                            },
                        };
                        Ok(SignedResponse::create(&response, &self.signing_key)?)
                    },
                    |reason, refund_transaction| {
                        Ok(SignedResponse::create(
                            &CreateSeatResponse {
                                quote_id,
                                outcome: CreateSeatOutcome::Refused {
                                    reason,
                                    refund_transaction: refund_transaction.cloned(),
                                },
                            },
                            &self.signing_key,
                        )?)
                    },
                )
                .await
                .map_err(|err| internal_chain("create_seat", &err))
        })
        .await
    }

    async fn get_dkg_code(
        &self,
        request: SignedRequest<GetDkgCodeRequest>,
    ) -> FmResult<GetDkgCodeResponse> {
        trace_formation_rpc("get_dkg_code", async {
            let request = self.validate(request)?;
            let seat = self
                .fleet
                .authorize(&request)
                .map_err(|err| map_seat_error("get_dkg_code", err))?;
            if let Some(name) = request.federation_name.as_ref() {
                validate_dkg_name("federation name", &name.0)?;
            }
            seat.reject_decommissioned()
                .map_err(|err| map_seat_error("get_dkg_code", err))?;
            let guardian_code = seat
                .dkg_code(request.federation_name.as_ref())
                .await
                .map_err(|err| map_seat_error("get_dkg_code", err))?;
            Ok(GetDkgCodeResponse { guardian_code })
        })
        .await
    }

    async fn start_dkg(
        &self,
        request: SignedRequest<StartDkgRequest>,
    ) -> FmResult<StartDkgResponse> {
        trace_formation_rpc("start_dkg", async {
            let request = self.validate(request)?;
            let seat = self
                .fleet
                .authorize(&request)
                .map_err(|err| map_seat_error("start_dkg", err))?;
            seat.reject_decommissioned()
                .map_err(|err| map_seat_error("start_dkg", err))?;
            let callback = request
                .completion_callback
                .as_ref()
                .map(|callback| self.validate_completion_callback(callback))
                .transpose()?;
            seat.start_dkg(&request.guardian_codes, callback)
                .await
                .map_err(|err| map_seat_error("start_dkg", err))?;
            Ok(StartDkgResponse)
        })
        .await
    }

    async fn restart_dkg(
        &self,
        request: SignedRequest<RestartDkgRequest>,
    ) -> FmResult<RestartDkgResponse> {
        trace_formation_rpc("restart_dkg", async {
            let request = self.validate(request)?;
            let seat = self
                .fleet
                .authorize(&request)
                .map_err(|err| map_seat_error("restart_dkg", err))?;
            let status = seat
                .restart_dkg(&request.guardian_codes)
                .await
                .map_err(|err| map_seat_error("restart_dkg", err))?;
            Ok(RestartDkgResponse { status })
        })
        .await
    }

    async fn get_status(
        &self,
        request: SignedRequest<GetStatusRequest>,
    ) -> FmResult<GetStatusResponse> {
        trace_formation_rpc("get_status", async {
            let request = self.validate(request)?;
            // No liveness gate: a decommissioned seat still answers status.
            let seat = self
                .fleet
                .authorize(&request)
                .map_err(|err| map_seat_error("get_status", err))?;
            let report = seat
                .report()
                .await
                .map_err(|err| map_seat_error("get_status", err))?;
            // Project onto the canonical wire status. The payment placeholders
            // (deadlines, grace) are 0.1 wire constants and live here, not in the
            // fleet.
            let (phase, health) = match report {
                SeatReport::Active { phase, health } => (phase, health),
                SeatReport::Decommissioned { at_ms } => {
                    return Ok(GetStatusResponse {
                        status: ServiceStatus::Decommissioned,
                        detail: StatusDetail::Ended(EndedStatusInfo {
                            reason: EndedReason::Decommissioned,
                            at: Timestamp(u64::try_from(at_ms / 1000).unwrap_or(0)),
                            note: None,
                        }),
                        seat_health: None,
                    });
                }
            };
            let (status, detail) = match phase {
                SeatPhase::Created => (ServiceStatus::New, StatusDetail::None),
                SeatPhase::DkgInProgress => (
                    ServiceStatus::DkgInProcess,
                    StatusDetail::Dkg(DkgStatusInfo {
                        peer_connections: Vec::new(),
                    }),
                ),
                SeatPhase::DataLoss { invite_code } => (
                    ServiceStatus::DataLoss,
                    StatusDetail::Federation(FederationStatusInfo {
                        valid_until_date: None,
                        invite_code: Some(invite_code),
                        peer_connections: Vec::new(),
                        in_grace: false,
                        grace_deadline: None,
                        stats: None,
                    }),
                ),
                SeatPhase::Running { invite_code } => (
                    ServiceStatus::Running,
                    StatusDetail::Federation(FederationStatusInfo {
                        valid_until_date: None,
                        invite_code: Some(invite_code),
                        peer_connections: Vec::new(),
                        in_grace: false,
                        grace_deadline: None,
                        stats: None,
                    }),
                ),
            };
            Ok(GetStatusResponse {
                status,
                detail,
                seat_health: Some(health),
            })
        })
        .await
    }

    async fn get_invite_code(
        &self,
        request: SignedRequest<GetInviteCodeRequest>,
    ) -> FmResult<GetInviteCodeResponse> {
        trace_formation_rpc("get_invite_code", async {
            let request = self.validate(request)?;
            let seat = self
                .fleet
                .authorize(&request)
                .map_err(|err| map_seat_error("get_invite_code", err))?;
            seat.reject_decommissioned()
                .map_err(|err| map_seat_error("get_invite_code", err))?;
            let invite_code = seat
                .invite_code()
                .await
                .map_err(|err| map_seat_error("get_invite_code", err))?;
            Ok(GetInviteCodeResponse { invite_code })
        })
        .await
    }

    async fn get_peer_attestation(
        &self,
        request: SignedRequest<GetPeerAttestationRequest>,
    ) -> FmResult<GetPeerAttestationResponse> {
        trace_formation_rpc("get_peer_attestation", async {
            let request = self.validate(request)?;
            let seat = self
                .fleet
                .authorize(&request)
                .map_err(|err| map_seat_error("get_peer_attestation", err))?;
            seat.reject_decommissioned()
                .map_err(|err| map_seat_error("get_peer_attestation", err))?;
            let binding = seat
                .federation_binding()
                .await
                .map_err(|err| map_seat_error("get_peer_attestation", err))?;

            let guardian_fee_account = self.fleet.guardian_fee_account_descriptor(&request.seat_id);
            let fman_peer_attestation = sign_peer_attestation(
                &self.attestation_keys,
                &binding,
                guardian_fee_account,
                now(),
            )
            .map_err(|err| internal("get_peer_attestation", err))?;
            let seat_endpoint_proof = seat
                .sign_endpoint_proof(fman_peer_attestation.attestation.clone())
                .await
                .map_err(|err| map_seat_error("get_peer_attestation", err))?;

            Ok(GetPeerAttestationResponse {
                fman_peer_attestation,
                seat_endpoint_proof,
            })
        })
        .await
    }

    async fn set_meta_field(
        &self,
        request: SignedRequest<SetMetaFieldRequest>,
    ) -> FmResult<SetMetaFieldResponse> {
        trace_formation_rpc("set_meta_field", async {
            let request = self.validate(request)?;
            let seat = self
                .fleet
                .authorize(&request)
                .map_err(|err| map_seat_error("set_meta_field", err))?;
            seat.reject_decommissioned()
                .map_err(|err| map_seat_error("set_meta_field", err))?;
            seat.submit_meta_field(
                request.expected_base,
                request.key.clone(),
                request.value.clone(),
                self.min_guardian_fee_ppm(),
                self.guardian_verification_fee_account.clone(),
            )
            .await
            .map_err(|err| map_seat_error("set_meta_field", err))?;
            Ok(SetMetaFieldResponse)
        })
        .await
    }

    async fn propose_formation_meta(
        &self,
        request: SignedRequest<ProposeFormationMetaRequest>,
    ) -> FmResult<ProposeFormationMetaResponse> {
        trace_formation_rpc("propose_formation_meta", async {
            let request = self.validate(request)?;
            let seat = self
                .fleet
                .authorize(&request)
                .map_err(|err| map_seat_error("propose_formation_meta", err))?;
            seat.reject_decommissioned()
                .map_err(|err| map_seat_error("propose_formation_meta", err))?;
            let guardian_verification_fee_account = self
                .guardian_verification_fee_account
                .clone()
                .ok_or(FleetManagerError::GuardianVerificationFeeAccountUnavailable)?;
            let guardian_verification_fee_account =
                GuardianFeeAccount::try_from(guardian_verification_fee_account)
                    .map_err(|_| FleetManagerError::GuardianVerificationFeeAccountUnavailable)?;
            if request.guardian_verification_fee_account != guardian_verification_fee_account {
                return Err(FleetManagerError::GuardianVerificationFeeAccountMismatch);
            }
            seat.propose_formation_meta(
                request.expected_base,
                request.seat_bindings.clone(),
                request.fi_fee_account.clone().into_account(),
                request.send_ppm,
                self.min_guardian_fee_ppm(),
                guardian_verification_fee_account.as_account().clone(),
            )
            .await
            .map_err(|err| map_seat_error("propose_formation_meta", err))?;
            Ok(ProposeFormationMetaResponse)
        })
        .await
    }

    async fn register_gateway(
        &self,
        request: SignedRequest<RegisterGatewayRequest>,
    ) -> FmResult<RegisterGatewayResponse> {
        let request = self.validate(request)?;
        let seat = self
            .fleet
            .authorize(&request)
            .map_err(|err| map_seat_error("register_gateway", err))?;
        seat.reject_decommissioned()
            .map_err(|err| map_seat_error("register_gateway", err))?;
        let gateway_api = SafeUrl::parse(request.gateway_api.as_str())
            .map_err(|_| FleetManagerError::InvalidGatewayApiUrl)?;
        let was_added = seat
            .register_gateway(gateway_api)
            .await
            .map_err(|err| map_seat_error("register_gateway", err))?;
        Ok(RegisterGatewayResponse { was_added })
    }

    async fn get_fedimint_stats(
        &self,
        request: SignedRequest<GetFedimintStatsRequest>,
    ) -> FmResult<GetFedimintStatsResponse> {
        let request = self.validate(request)?;
        let _seat = self
            .fleet
            .authorize(&request)
            .map_err(|err| map_seat_error("get_fedimint_stats", err))?;
        unsupported("GetFedimintStats")
    }

    /// Serve this FMan's signed trust material for one federation.
    ///
    /// Unauthenticated by design: this is the material every verifier holding
    /// an invite code is meant to be able to fetch, so there is no requester to
    /// authorize and no seat the caller names. The request instead names a
    /// federation, and the answer covers only seats this FMan actually runs in
    /// it — an FMan that runs none answers with an empty attestation list
    /// rather than an error, because "I am not in that federation" is a fact
    /// about the federation, not a failure to serve.
    ///
    /// The response is signed by the service Nostr key, the same identity that
    /// signs the kind-37701 advertisement and every peer attestation, because
    /// a verifier resolves this FMan *by* that pubkey.
    async fn get_federation_trust_material(
        &self,
        request: GetFederationTrustMaterialRequest,
    ) -> FmResult<GetFederationTrustMaterialResponse> {
        request.validate().map_err(|err| {
            tracing::warn!(error = %err, "rejecting malformed trust-material request");
            FleetManagerError::Other("invalid trust-material request".to_owned())
        })?;

        let Some(source) = self.trust_material.get() else {
            // No Nostr relay is configured, so this FMan has never learned a
            // holder authorization and cannot produce trust material at all.
            // Answering `unsupported` rather than an empty document keeps a
            // verifier from reading "no authorizations" as "untrusted FMan"
            // when the truth is "this FMan is not participating".
            return unsupported("GetFederationTrustMaterial");
        };

        let bindings = self
            .fleet
            .federation_bindings(&request.federation_id, &request.federation_config_hash)
            .await;

        let filter: std::collections::BTreeSet<_> = request.peer_ids.iter().collect();
        let issued_at = now();
        let mut peer_attestations = Vec::new();
        for binding in &bindings {
            if !filter.is_empty() && !filter.contains(&binding.seat.peer_id) {
                continue;
            }
            peer_attestations.push(
                sign_peer_attestation(
                    &self.attestation_keys,
                    binding,
                    self.fleet.guardian_fee_account_descriptor(&binding.seat_id),
                    issued_at,
                )
                .map_err(|err| internal("get_federation_trust_material", err))?,
            );
        }

        let material = FmanFederationTrustMaterial {
            fman_pubkey: Pubkey(self.attestation_keys.public_key().to_string()),
            federation_id: request.federation_id.clone(),
            federation_config_hash: request.federation_config_hash.clone(),
            issued_at,
            expires_at: Timestamp(issued_at.0.saturating_add(TRUST_MATERIAL_VALIDITY_SECS)),
            public_api_urls: vec![source.iroh_endpoint_url()],
            peer_attestations,
            holder_authorizations: source.holder_authorizations(),
        };

        let digest = material
            .digest()
            .map_err(|err| internal("get_federation_trust_material", err))?;
        let signature = self
            .attestation_keys
            .sign_schnorr(&nostr_sdk::secp256k1::Message::from_digest(digest));

        Ok(GetFederationTrustMaterialResponse {
            version: ProtocolV1,
            material,
            proof: SchnorrSignatureProof { signature },
        })
    }
}

fn is_local_single_guardian_e2e(size: u16) -> bool {
    size == 1 && std::env::var_os("FMAN_E2E_LOCAL_IROH").is_some()
}

#[cfg(test)]
#[path = "../tests/service.rs"]
mod tests;
