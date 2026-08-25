//! The federation trust pipeline.
//!
//! A `RequestLiquidity` is admitted only if the invite previews to the endorsed
//! federation, the consensus seat bindings name FMans the request carries trust
//! material for, that material verifies against a trusted issuer, and no
//! required revocation lookup fails. A dependency that cannot be reached fails
//! the request closed rather than being bypassed. See
//! [SPEC-flip-federation-trust](../specs/SPEC-flip-federation-trust.md).

use std::collections::{BTreeMap, BTreeSet};
use std::str::FromStr as _;
use std::sync::Arc;

use async_trait::async_trait;
use fedi_decentralized_federation_preview::FederationPreview;
use fedi_decentralized_service_liquidity_manager::{
    CredentialDigest, FederationId, FmanFederationTrustMaterial, FmanSeatBindings,
    GetFederationTrustMaterialRequest, HolderAuthorizationEnvelope, HolderTrustEnvelopeError,
    PeerBadgeTrustPolicy, PeerBadgeTrustPolicyError, ProtocolV1, Pubkey, PublicRejection,
    PublicRejectionCode, RequestLiquidityRequest, SetupConfigView, Timestamp, TrustScoreBadgeV1,
    VerificationCheck, VerificationCheckStatus, VerificationContext, VerificationRequirement,
    VerificationSummary, VerifiedSeatBinding, verify_holder_trust_envelope,
};

use crate::database::Database;
use crate::federation_preview::{FederationPreviewError, FederationPreviewProvider};
use crate::now_timestamp;
use crate::revocation::{RevocationFetcher, credential_digest_wire_string, run_revocation_stage};
use crate::stability_pool::STABILITY_POOL_MODULE_KIND;
use crate::verification_budget::VerificationBudget;

fn preview_error_rejection(error: FederationPreviewError) -> (PublicRejectionCode, String) {
    match error {
        FederationPreviewError::InvalidInviteCode(reason) => (
            PublicRejectionCode::InvalidDetailsPayload,
            format!("invite code preview failed: {reason}"),
        ),
        FederationPreviewError::EndpointPolicyRejected => (
            PublicRejectionCode::InvalidDetailsPayload,
            "invite endpoint rejected by transport policy".to_owned(),
        ),
        FederationPreviewError::Unavailable(_) => (
            PublicRejectionCode::ProviderUnavailable,
            "federation preview is unavailable".to_owned(),
        ),
    }
}

/// Longest `expires_at - issued_at` window FLIP accepts on FMan trust material.
///
/// The requester collects material for a request it is about to send, so a long
/// window buys it nothing. What the bound is actually for: with the live
/// advertisement lookup gone, nothing else tells FLIP that an FMan is *still*
/// operating. An FMan that goes dark stays trusted until its last material
/// expires, so this is how long that shadow lasts. Revocation still runs live,
/// so a withdrawn badge is caught inside the window regardless.
const FLIP_TRUST_MATERIAL_MAX_VALIDITY_SECS: u64 = 3600;

/// One operating identity's verified trust material.
struct ResolvedTrustMaterial {
    fman_pubkey: Pubkey,
    holder_authorizations: Vec<HolderAuthorizationEnvelope>,
}

/// Complete FMan badge authentication plus the selected relying-party policy.
#[derive(Debug, thiserror::Error)]
enum FmanPeerBadgeError {
    #[error(transparent)]
    Envelope(#[from] HolderTrustEnvelopeError),

    #[error(transparent)]
    TrustPolicy(#[from] PeerBadgeTrustPolicyError),
}

/// Check an FMan's own peer attestations against the seat-binding directory.
///
/// Both describe which seats this FMan operates, and they are produced
/// independently — the directory reached consensus among threshold guardians,
/// the material was signed by this FMan alone. Agreement is expected; a
/// disagreement means one of them is describing a federation it is not in, and
/// the directory is the one with a federation behind it.
///
/// Only the seats the material claims are checked. An FMan may legitimately
/// answer for fewer seats than it holds (the request's peer filter allows
/// exactly that), so a missing claim is not a contradiction — a *wrong* one is.
fn cross_check_attestations(
    material: &FmanFederationTrustMaterial,
    verified_bindings: &[VerifiedSeatBinding],
) -> Result<(), String> {
    for attestation in &material.peer_attestations {
        let statement = attestation
            .verify()
            .map_err(|error| format!("nested peer attestation does not verify: {error}"))?;
        let Some(binding) = verified_bindings
            .iter()
            .find(|binding| binding.peer_id == statement.peer_id)
        else {
            return Err(format!(
                "material claims peer {} which the directory does not name",
                statement.peer_id.0
            ));
        };
        if binding.fman_pubkey != statement.fman_pubkey {
            return Err(format!(
                "material claims peer {} for {}, but the directory binds it to {}",
                statement.peer_id.0, statement.fman_pubkey.0, binding.fman_pubkey.0
            ));
        }
        if binding.guardian_identity != statement.guardian_identity {
            return Err(format!(
                "material claims a different guardian identity for peer {}",
                statement.peer_id.0
            ));
        }
        if binding.guardian_fee_account != statement.guardian_fee_account {
            return Err(format!(
                "material claims a different guardian-fee account for peer {}",
                statement.peer_id.0
            ));
        }
    }

    Ok(())
}

/// The one mode name the trust pipeline reports.
///
/// Deliberately not an enum: there is exactly one, and both trust-input
/// variants report it. What distinguishes them is
/// [`VerificationModeInfo::fixtures`], not the name.
const TRUST_PIPELINE_MODE: &str = "trust_pipeline";

/// Operator-visible verification mode.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct VerificationModeInfo {
    /// Stable machine-readable mode name.
    pub mode: &'static str,

    /// Whether the pipeline's trust inputs (federation preview, FMan trust
    /// material) can produce verification results.
    pub inputs_available: bool,

    /// Whether the trust inputs come from `--trust-fixtures` files instead of
    /// the real network boundaries. Loudly surfaced in health output; never a
    /// production trust configuration.
    pub fixtures: bool,

    /// Operator-readable detail.
    pub detail: &'static str,
}

/// Source of the pipeline's substitutable trust inputs (federation preview +
/// FMan trust material). The revocation fetcher is never substituted.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TrustInputs {
    /// Every trust input is the real network path.
    Production,

    /// The invite-code preview comes from `--trust-fixtures` files; test
    /// deployments only. Advertisements and revocations stay real.
    Fixtures,
}

/// Result of verifying one liquidity request.
#[derive(Clone, Debug)]
pub(crate) struct VerificationOutcome {
    /// Persisted per-request verification summary.
    pub summary: VerificationSummary,

    /// Signed protocol rejection when verification gates acceptance.
    pub rejection: Option<PublicRejection>,
}

/// Private federation and credential verification boundary.
///
/// `verify` may perform bounded network I/O (relay revocation lookups,
/// federation preview), so callers must never invoke it while holding an open
/// database transaction. `Database::begin_write` takes SQLite's single write
/// lock up front, so doing that would stall every other writer in the process
/// for a relay round trip; that end carries the same rule. FMan trust material
/// is carried in the request rather than fetched.
#[async_trait]
pub(crate) trait VerificationProvider: Send + Sync {
    fn mode(&self) -> VerificationModeInfo;

    /// Verify a liquidity request against current operator config.
    async fn verify(
        &self,
        request: &RequestLiquidityRequest,
        config: &SetupConfigView,
    ) -> VerificationOutcome;
}

/// Runtime dependencies for the verification pipeline.
pub(crate) struct VerificationDeps {
    pub database: Database,
    pub revocation_fetcher: Arc<dyn RevocationFetcher>,
    pub preview_provider: Arc<dyn FederationPreviewProvider>,
    pub verification_budget: Arc<VerificationBudget>,
}

/// The trust verification pipeline.
///
/// The pipeline implements the settled verification flow: admission gate,
/// invite-code preview, seat bindings read from `fedi:fman_seat_bindings` and
/// matched against the previewed config, request-carried FMan trust material
/// verified per operating identity, holder trust envelope verification with
/// fresh fail-closed revocation lookups, the selected environment's minimum
/// badge policy, and accepted-attester policy evaluation over distinct FMan
/// identities.
///
/// Trust standing arrives as signed material inside the request rather than
/// from a relay lookup. The requester carries it but does not author it: the
/// consensus seat-binding directory decides which identities exist, and each
/// identity's material is signed by that identity, so the requester's only
/// influence is whether an identity is answered for — and an unanswered one is
/// untrusted. Revocation is still resolved live, so a withdrawn badge is caught
/// regardless of what the request carries.
///
/// Only the Fedimint preview stays behind a substitutable boundary trait, via
/// `--trust-fixtures` for test deployments.
pub(crate) struct VerificationPipeline {
    deps: VerificationDeps,
    trust_inputs: TrustInputs,
    peer_badge_trust_policy: PeerBadgeTrustPolicy,
}

impl VerificationPipeline {
    pub(crate) fn new(
        deps: VerificationDeps,
        trust_inputs: TrustInputs,
        peer_badge_trust_policy: PeerBadgeTrustPolicy,
    ) -> Self {
        Self {
            deps,
            trust_inputs,
            peer_badge_trust_policy,
        }
    }

    fn verify_fman_peer_badge(
        &self,
        verifier: &VerificationContext,
        envelope: &HolderAuthorizationEnvelope,
        expected_subject: &Pubkey,
        now: Timestamp,
    ) -> Result<TrustScoreBadgeV1, FmanPeerBadgeError> {
        let badge = verify_holder_trust_envelope(verifier, envelope, expected_subject, now)?;
        self.peer_badge_trust_policy.require(&badge)?;
        Ok(badge)
    }

    /// Admit or reject a request on its `fman_endorsement`, before previewing.
    ///
    /// Five checks and deliberately no more: the attestation's own signature,
    /// that it names the federation this request is about, that its FMan
    /// identity holds a badge from a trusted issuer, that the badge meets the
    /// selected environment's minimum level, and that the badge is not revoked.
    /// Possession of an endorsement is the authorization, so there is
    /// no requester binding, allowlist, quota, or freshness window here — that
    /// is the settled model, not an omission
    /// ([`SPEC-flip-rpc`]). The gate is never
    /// authoritative for per-guardian policy either; the pipeline below still
    /// verifies every seat binding and resolves every operating identity.
    ///
    /// The federation is taken from the invite code rather than from
    /// `federation_details.federation_id`, which the requester supplies and
    /// could set to match any endorsement it happens to hold.
    ///
    /// [`SPEC-flip-rpc`]: https://github.com/fedibtc/manifold/blob/master/crates/liquidity-manager-daemon/specs/SPEC-flip-rpc.md
    async fn run_admission_gate(
        &self,
        request: &RequestLiquidityRequest,
        checks: &mut PipelineChecks,
    ) -> Result<FederationId, (PublicRejectionCode, String)> {
        let Some(endorsement) = request.fman_endorsement.as_ref() else {
            return Err((
                PublicRejectionCode::InvalidCredentials,
                "request carries no FMan endorsement".to_owned(),
            ));
        };

        let statement = endorsement.attestation.verify().map_err(|error| {
            (
                PublicRejectionCode::InvalidCredentials,
                format!("FMan endorsement attestation failed verification: {error}"),
            )
        })?;

        let invite = &request.federation_details.invite_code.0;
        let federation_id = fedimint_core::invite_code::InviteCode::from_str(invite)
            .map(|invite| invite.federation_id().to_string())
            .map_err(|error| {
                (
                    PublicRejectionCode::InvalidDetailsPayload,
                    format!("invite code is malformed: {error}"),
                )
            })?;
        if statement.federation_id.0 != federation_id {
            return Err((
                PublicRejectionCode::InvalidSeatBinding,
                "FMan endorsement is for a different federation than the invite code".to_owned(),
            ));
        }

        let authorities = crate::attestation_store::trusted_issuer_authorities(&self.deps.database)
            .await
            .map_err(|error| {
                (
                    PublicRejectionCode::ProviderUnavailable,
                    format!("trusted issuer authorities are unavailable: {error}"),
                )
            })?;
        let digest = &endorsement
            .trust
            .holder_authorization
            .authorization
            .credential_digest;
        let issuer_hex = endorsement
            .trust
            .signed_credential
            .credential
            .issuer_id_pubkey
            .0
            .to_string();
        if !authorities
            .iter()
            .any(|authority| authority.issuer.issuer_id_pubkey.0.to_string() == issuer_hex)
        {
            return Err((
                PublicRejectionCode::InvalidCredentials,
                format!("FMan endorsement badge issuer {issuer_hex} is not trusted here"),
            ));
        }

        let mut verifier = VerificationContext::new();
        for authority in &authorities {
            verifier.add_issuer_authority(authority).map_err(|error| {
                (
                    PublicRejectionCode::ProviderUnavailable,
                    format!("trusted issuer authority failed to load: {error}"),
                )
            })?;
        }
        // The badge cryptography runs before the revocation lookup, and the
        // order is load-bearing rather than incidental. Everything checked
        // above is forgeable by anyone holding the invite code and a trusted
        // issuer's pubkey: the attestation is self-signable, and the
        // issuer-installed check reads the credential's *claimed* issuer id,
        // which nothing has verified yet. Looking up revocations first would
        // let such a forgery cost the daemon a relay round trip before any
        // cryptography rejected it.
        //
        // This binds the badge to the attestation too: the envelope's subject
        // must be the FMan identity that signed the seat binding, so an
        // endorsement cannot pair one guardian's attestation with another's
        // badge.
        let verify = |verifier: &VerificationContext, now| {
            self.verify_fman_peer_badge(verifier, &endorsement.trust, &statement.fman_pubkey, now)
                .map_err(|error| {
                    (
                        PublicRejectionCode::InvalidCredentials,
                        format!("FMan endorsement badge failed verification: {error}"),
                    )
                })
        };
        verify(&verifier, now_timestamp())?;

        // Everything needed to make the invite-derived federation id a scarce,
        // authenticated budget key is now verified locally. Charge before the
        // first outbound stage (the live revocation lookup), but never use the
        // requester-declared federation id, which remains untrusted until preview.
        if !self
            .deps
            .verification_budget
            .try_spend(&federation_id, std::time::Instant::now())
        {
            return Err((
                PublicRejectionCode::ProviderUnavailable,
                "verification allowance for this federation is spent; retry later".to_owned(),
            ));
        }

        // Only now is the lookup worth its cost. It feeds revocations into the
        // verifier, which is what makes the second pass able to fail a badge
        // that is otherwise entirely valid.
        let stage = run_revocation_stage(
            self.deps.revocation_fetcher.as_ref(),
            &mut verifier,
            &authorities,
            &[(issuer_hex.clone(), digest.clone())],
        )
        .await;
        checks.revocation.extend(stage.checks);
        if stage.unavailable {
            return Err((
                PublicRejectionCode::ProviderUnavailable,
                "the FMan endorsement's revocation lookup could not complete freshly".to_owned(),
            ));
        }
        let badge = verify(&verifier, now_timestamp())?;

        checks.credential.push(check(
            "fman_endorsement",
            VerificationCheckStatus::Passed,
            Some(statement.fman_pubkey.0.clone()),
            format!(
                "endorsement for peer {} verified against issuer {issuer_hex} with trust level {}",
                statement.peer_id.0, badge.trust_level
            ),
        ));

        Ok(FederationId(federation_id))
    }

    /// Resolve every operating identity from the request-carried trust
    /// material.
    ///
    /// One entry per identity the *directory* names. An identity with no entry
    /// is left untrusted and reaches the policy stage as `policy_mismatch`; an
    /// entry that is present but does not verify is `invalid_credentials`,
    /// because a requester that carried material at all is claiming it is good.
    ///
    /// Note what is *not* checked here: nothing requires the material to cover
    /// only the directory's identities. Extra entries are ignored rather than
    /// refused, since an entry for an identity this federation does not name
    /// can never be consulted.
    fn resolve_trust_material(
        &self,
        request: &RequestLiquidityRequest,
        preview: &FederationPreview,
        operators: &BTreeSet<String>,
        verified_bindings: &[VerifiedSeatBinding],
        now: fedi_decentralized_service_liquidity_manager::Timestamp,
        checks: &mut PipelineChecks,
    ) -> Result<Vec<ResolvedTrustMaterial>, (PublicRejectionCode, String)> {
        let Some(material) = request.fman_trust_material.as_deref() else {
            return Err((
                PublicRejectionCode::InvalidCredentials,
                "request carries no FMan trust material".to_owned(),
            ));
        };

        // Two entries for one identity would make "the material for this FMan"
        // ambiguous, and resolving that by position would let the ordering of a
        // requester-supplied list decide a trust outcome. Refuse it the way the
        // seat-binding container refuses a repeated peer id.
        let mut seen: BTreeSet<&str> = BTreeSet::new();
        for response in material {
            if !seen.insert(response.material.fman_pubkey.0.as_str()) {
                return Err((
                    PublicRejectionCode::InvalidCredentials,
                    format!(
                        "request carries more than one trust material entry for {}",
                        response.material.fman_pubkey.0
                    ),
                ));
            }
        }

        // Built from the preview, never from `federation_details`, for the same
        // reason the admission gate reads the invite code: the requester
        // supplies both, and only the preview is derived from the federation.
        let material_request = GetFederationTrustMaterialRequest {
            version: ProtocolV1,
            federation_id: preview.federation_id.clone(),
            federation_config_hash: preview.federation_config_hash.clone(),
            peer_ids: Vec::new(),
        };

        let mut resolved = Vec::new();
        for fman_pubkey in operators {
            let Some(response) = material
                .iter()
                .find(|response| response.material.fman_pubkey.0 == *fman_pubkey)
            else {
                checks.credential.push(check(
                    "fman_trust_material",
                    VerificationCheckStatus::Failed,
                    Some(fman_pubkey.clone()),
                    "request carries no trust material for this FMan identity",
                ));
                continue;
            };

            let verified = response
                .verify_for_request(
                    &material_request,
                    now,
                    FLIP_TRUST_MATERIAL_MAX_VALIDITY_SECS,
                )
                .map_err(|error| {
                    (
                        PublicRejectionCode::InvalidCredentials,
                        format!("FMan trust material failed verification: {error}"),
                    )
                })?;

            // The material's own attestations are cross-checked against the
            // directory rather than trusted in its place. `verify_for_request`
            // has already proven each one is signed by this FMan and names this
            // federation and config revision; what it cannot know is which
            // seats the federation actually assigned. Disagreement means one of
            // the two is describing a different federation than it claims to.
            cross_check_attestations(&verified, verified_bindings).map_err(|reason| {
                (
                    PublicRejectionCode::InvalidCredentials,
                    format!("FMan trust material contradicts the seat-binding directory: {reason}"),
                )
            })?;

            checks.credential.push(check(
                "fman_trust_material",
                VerificationCheckStatus::Passed,
                Some(fman_pubkey.clone()),
                "signed trust material verified against the previewed federation",
            ));
            resolved.push(ResolvedTrustMaterial {
                fman_pubkey: Pubkey(fman_pubkey.clone()),
                holder_authorizations: verified.holder_authorizations,
            });
        }

        Ok(resolved)
    }

    async fn run_pipeline(
        &self,
        request: &RequestLiquidityRequest,
        config: &SetupConfigView,
        checks: &mut PipelineChecks,
    ) -> Result<PolicySuccess, (PublicRejectionCode, String)> {
        let now = now_timestamp();
        let details = &request.federation_details;

        // 0. Admission gate. Runs before the preview so an unauthorized
        //    request costs no network I/O.
        let admitted_federation_id = self.run_admission_gate(request, checks).await?;

        // 1. Preview the invite code for the authoritative federation facts.
        let preview = self
            .deps
            .preview_provider
            .preview(&details.invite_code)
            .await
            .map_err(preview_error_rejection)?;

        // 2. FI-provided hints are non-authoritative but must not contradict
        //    the preview.
        if preview.federation_id != admitted_federation_id
            || details.federation_id != preview.federation_id
            || details.federation_config_hash != preview.federation_config_hash
            || request.network != preview.network
        {
            return Err((
                PublicRejectionCode::InvalidDetailsPayload,
                "request federation details do not match the invite-code preview".to_owned(),
            ));
        }
        for hint in &details.fleet_seat_hints {
            let matches_preview = preview.peers.iter().any(|peer| {
                peer.peer_id == hint.peer_id && peer.guardian_identity == hint.guardian_identity
            });
            if !matches_preview {
                return Err((
                    PublicRejectionCode::InvalidDetailsPayload,
                    "fleet seat hint does not match the invite-code preview".to_owned(),
                ));
            }
        }

        // 2a. A source the target federation cannot serve is refused here, not
        //     discovered by the worker. Nothing downstream reads the module
        //     map, so without this a federation formed without the optional
        //     stability-pool module was accepted for a stability allocation,
        //     funded through an ordinary wallet peg-in, and only failed at the
        //     first `StabilityPoolClientModule` lookup — which happens after
        //     the peg-in is claimed, leaving provider e-cash in a target client
        //     that can never deposit it.
        //
        //     Bound to the previewed config, which step 2 has just tied to the
        //     hash the request carries, so this answer is about the same
        //     configuration the allocation is recorded against.
        if request.amounts.stability_min_amount.0 > 0
            && !preview.module_kinds.contains(STABILITY_POOL_MODULE_KIND)
        {
            return Err((
                PublicRejectionCode::UnsupportedSourceType,
                "target federation's configuration has no stability-pool module".to_owned(),
            ));
        }

        // 3. Read the seat-binding directory from consensus metadata and match
        //    it to the previewed config. The shared container owns every
        //    structural and against-the-config rule, so this stage only maps
        //    its errors onto rejection codes.
        let Some(metadata_value) = preview.fman_seat_bindings_metadata.as_deref() else {
            return Err((
                PublicRejectionCode::InvalidSeatBinding,
                "fedi:fman_seat_bindings consensus metadata is missing".to_owned(),
            ));
        };
        let bindings = FmanSeatBindings::parse_canonical(metadata_value).map_err(|error| {
            (
                PublicRejectionCode::InvalidSeatBinding,
                format!("fedi:fman_seat_bindings consensus metadata is invalid: {error}"),
            )
        })?;
        let verified_bindings = bindings
            .verify_for_federation(&preview.federation_seats())
            .map_err(|error| {
                (
                    PublicRejectionCode::InvalidSeatBinding,
                    format!("FMan seat bindings do not match the previewed config: {error}"),
                )
            })?;
        for binding in &verified_bindings {
            checks.seat.push(check(
                "seat_binding",
                VerificationCheckStatus::Passed,
                Some(binding.fman_pubkey.0.clone()),
                format!("peer {} is bound to this FMan identity", binding.peer_id.0),
            ));
        }

        // 4. Resolve each distinct operating identity from the request-carried
        //    trust material. One FMan may hold several seats; it is resolved
        //    once and counts once.
        //
        //    The identity set comes from the *meta* directory verified above,
        //    never from the material itself. That ordering is the whole safety
        //    argument for accepting requester-carried trust: the federation
        //    decides who its operators are, and the material is only ever
        //    consulted for an identity the directory already named.
        let operators: BTreeSet<String> = verified_bindings
            .iter()
            .map(|binding| binding.fman_pubkey.0.clone())
            .collect();
        let advertisements = self.resolve_trust_material(
            request,
            &preview,
            &operators,
            &verified_bindings,
            now,
            checks,
        )?;

        // 6. Pair and verify holder trust envelopes per distinct FMan
        //    identity, with fresh fail-closed revocation lookups.
        let authorities = crate::attestation_store::trusted_issuer_authorities(&self.deps.database)
            .await
            .map_err(|error| {
                (
                    PublicRejectionCode::ProviderUnavailable,
                    format!("trusted issuer authorities are unavailable: {error}"),
                )
            })?;
        let installed_issuers: BTreeSet<String> = authorities
            .iter()
            .map(|authority| authority.issuer.issuer_id_pubkey.0.to_string())
            .collect();
        let accepted_attesters: BTreeSet<&str> = config
            .policy
            .accepted_attester_policies
            .iter()
            .map(|policy| policy.attester_pubkey.0.as_str())
            .collect();

        let mut candidates: Vec<(
            String,
            String,
            CredentialDigest,
            HolderAuthorizationEnvelope,
        )> = Vec::new();
        for advertisement in &advertisements {
            let fman_pubkey = advertisement.fman_pubkey.0.clone();
            for envelope in &advertisement.holder_authorizations {
                // The envelope carries the authorization and its backing
                // credential together, and `verify_holder_trust_envelope`
                // checks the digest binding between them.
                let digest = envelope
                    .holder_authorization
                    .authorization
                    .credential_digest
                    .clone();
                let issuer_hex = envelope
                    .signed_credential
                    .credential
                    .issuer_id_pubkey
                    .0
                    .to_string();
                if !accepted_attesters.contains(issuer_hex.as_str()) {
                    checks.credential.push(check(
                        "issuer_credential_policy",
                        VerificationCheckStatus::NotRun,
                        Some(fman_pubkey.clone()),
                        format!("issuer {issuer_hex} is not an accepted attester; ignored"),
                    ));
                    continue;
                }
                if !installed_issuers.contains(&issuer_hex) {
                    checks.credential.push(check(
                        "issuer_credential_policy",
                        VerificationCheckStatus::Failed,
                        Some(fman_pubkey.clone()),
                        format!("no trusted issuer authority is installed for {issuer_hex}"),
                    ));
                    continue;
                }
                candidates.push((fman_pubkey.clone(), issuer_hex, digest, envelope.clone()));
            }
        }

        let mut verifier = VerificationContext::new();
        for authority in &authorities {
            verifier.add_issuer_authority(authority).map_err(|error| {
                (
                    PublicRejectionCode::ProviderUnavailable,
                    format!("trusted issuer authority failed to load: {error}"),
                )
            })?;
        }
        let mut required: Vec<(String, CredentialDigest)> = Vec::new();
        for (_, issuer_hex, digest, _) in &candidates {
            let pair = (issuer_hex.clone(), digest.clone());
            if !required.contains(&pair) {
                required.push(pair);
            }
        }
        let stage = run_revocation_stage(
            self.deps.revocation_fetcher.as_ref(),
            &mut verifier,
            &authorities,
            &required,
        )
        .await;
        checks.revocation.extend(stage.checks);
        if stage.unavailable {
            return Err((
                PublicRejectionCode::ProviderUnavailable,
                "a required issuer revocation lookup could not complete freshly".to_owned(),
            ));
        }

        let mut trusted: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for (fman_pubkey, issuer_hex, digest, envelope) in &candidates {
            match self.verify_fman_peer_badge(
                &verifier,
                envelope,
                &Pubkey(fman_pubkey.clone()),
                now,
            ) {
                Ok(badge) => {
                    trusted
                        .entry(issuer_hex.clone())
                        .or_default()
                        .insert(fman_pubkey.clone());
                    checks.credential.push(check(
                        "issuer_credential_policy",
                        VerificationCheckStatus::Passed,
                        Some(fman_pubkey.clone()),
                        format!(
                            "trust badge from {issuer_hex} verified with trust level {}",
                            badge.trust_level
                        ),
                    ));
                }
                Err(error) => {
                    checks.credential.push(check(
                        "issuer_credential_policy",
                        VerificationCheckStatus::Failed,
                        Some(fman_pubkey.clone()),
                        format!(
                            "trust badge {} failed verification: {error}",
                            credential_digest_wire_string(digest)
                        ),
                    ));
                    return Err((
                        PublicRejectionCode::InvalidCredentials,
                        format!("fetched FMan credential failed verification: {error}"),
                    ));
                }
            }
        }

        // 7. Evaluate accepted attester policies over distinct identities.
        for policy in &config.policy.accepted_attester_policies {
            let trusted_for_attester = trusted
                .get(policy.attester_pubkey.0.as_str())
                .cloned()
                .unwrap_or_default();
            let trusted_operators = operators
                .iter()
                .filter(|identity| trusted_for_attester.contains(*identity))
                .count();
            let satisfied = match policy.verification_requirement {
                VerificationRequirement::AllTrusted => trusted_operators == operators.len(),
                VerificationRequirement::ConsensusMajorityTrusted => {
                    trusted_operators >= preview.consensus_threshold as usize
                }
            };
            if satisfied {
                return Ok(PolicySuccess {
                    policy: policy.clone(),
                });
            }
        }

        Err((
            PublicRejectionCode::PolicyMismatch,
            "no accepted attester policy is satisfied by the distinct trusted FMan identities"
                .to_owned(),
        ))
    }
}

#[async_trait]
impl VerificationProvider for VerificationPipeline {
    fn mode(&self) -> VerificationModeInfo {
        match self.trust_inputs {
            TrustInputs::Production => VerificationModeInfo {
                mode: TRUST_PIPELINE_MODE,
                inputs_available: true,
                fixtures: false,
                detail: "the production trust pipeline is running against the real invite-code preview, the FMan trust material the request carries, and revocation relays",
            },
            TrustInputs::Fixtures => VerificationModeInfo {
                mode: TRUST_PIPELINE_MODE,
                inputs_available: true,
                fixtures: true,
                detail: "the production trust pipeline is running with a --trust-fixtures file substitute for the invite-code preview; request-carried FMan trust material is verified in full and revocations still use the real relays; test deployments only",
            },
        }
    }

    async fn verify(
        &self,
        request: &RequestLiquidityRequest,
        config: &SetupConfigView,
    ) -> VerificationOutcome {
        let mut checks = PipelineChecks::default();
        let outcome = match self.run_pipeline(request, config, &mut checks).await {
            Ok(success) => VerificationOutcome {
                summary: VerificationSummary {
                    federation_id: request.federation_details.federation_id.clone(),
                    policy_result: VerificationCheckStatus::Passed,
                    seat_checks: checks.seat,
                    credential_checks: checks.credential,
                    revocation_checks: checks.revocation,
                    accepted_attester_policy: Some(success.policy),
                    failure_reason: None,
                },
                rejection: None,
            },
            Err((code, reason)) => VerificationOutcome {
                summary: VerificationSummary {
                    federation_id: request.federation_details.federation_id.clone(),
                    policy_result: VerificationCheckStatus::Failed,
                    seat_checks: checks.seat,
                    credential_checks: checks.credential,
                    revocation_checks: checks.revocation,
                    accepted_attester_policy: None,
                    failure_reason: Some(reason.clone()),
                },
                rejection: Some(PublicRejection {
                    code,
                    reason: Some(reason),
                }),
            },
        };
        // The admission or refusal is already one line at the public boundary.
        // What that line cannot carry is which stages ran and which one refused,
        // and this is the only place holding both. It is debug because the
        // caller chooses how often it happens.
        let failed_checks = outcome
            .summary
            .seat_checks
            .iter()
            .chain(outcome.summary.credential_checks.iter())
            .chain(outcome.summary.revocation_checks.iter())
            .filter(|check| check.status != VerificationCheckStatus::Passed)
            .map(|check| check.name.as_str())
            .collect::<Vec<_>>()
            .join(",");
        tracing::debug!(
            federation_id = %request.federation_details.federation_id.0,
            policy_result = %outcome.summary.policy_result,
            seat_checks = outcome.summary.seat_checks.len(),
            credential_checks = outcome.summary.credential_checks.len(),
            revocation_checks = outcome.summary.revocation_checks.len(),
            failed_checks,
            "verification pipeline finished"
        );
        outcome
    }
}

struct PolicySuccess {
    policy: fedi_decentralized_service_liquidity_manager::AcceptedAttesterPolicy,
}

#[derive(Default)]
struct PipelineChecks {
    seat: Vec<VerificationCheck>,
    credential: Vec<VerificationCheck>,
    revocation: Vec<VerificationCheck>,
}

fn check(
    name: impl Into<String>,
    status: VerificationCheckStatus,
    subject: Option<String>,
    detail: impl Into<String>,
) -> VerificationCheck {
    VerificationCheck {
        name: name.into(),
        status,
        subject,
        detail: Some(detail.into()),
    }
}

#[cfg(test)]
#[path = "../tests/verification.rs"]
mod tests;
