//! Public Liquidity API: request acceptance, and the Iroh transport serving it.
//!
//! This is the app-facing surface. A request is verified, planned against
//! current capacity, and committed as an allocation under the setup revision
//! that admitted it; a rejection persists nothing, so a retry re-evaluates from
//! scratch. The transport binds a key derived from the provider identity, so
//! the advertised node id survives a restart. See
//! [SPEC-flip-rpc](../specs/SPEC-flip-rpc.md).

use anyhow::Context;
use fedi_decentralized_service_liquidity_manager::{
    AcceptedAttesterPolicy, AllocationItemStatus, AllocationItemTarget, AllocationStatus,
    CapacityMode, GetAllocationStatusRequest, GetAllocationStatusResponse, GetProviderInfoRequest,
    GetProviderInfoResponse, ItemAllocationStatus, LiquidityAmountBounds,
    PUBLIC_LIQUIDITY_API_ALPN, PUBLIC_LIQUIDITY_PROTOCOL_VERSION as PROTOCOL_VERSION,
    ProviderInfoOutcome, ProviderPolicy, PublicLiquidityApi, PublicLiquidityApiServer,
    PublicRejection, PublicRejectionCode, RequestLiquidityOutcome, RequestLiquidityRequest,
    RequestLiquidityResponse, RpcEndpointId, Sats, ServiceResult, SetupConfigView, SetupStatus,
    Signed, SourceType, Timestamp, request_liquidity_details_hash_for_request,
};
use fedi_iroh_rpc::IrohProtocol;
use fedi_iroh_rpc::iroh::{Endpoint, SecretKey, endpoint::presets, protocol::Router};
use sqlx::{Row, Sqlite, Transaction};

use crate::DaemonContext;
use crate::advertisement;
use crate::allocation_store::{self, PlannedItem};
use crate::auth::PublicAuthProvider;
use crate::daemon::AllocationAdmission;
use crate::database::Database;
use crate::funds_admin;
use crate::identity;
use crate::setup_store::{self, StoredSetupState};
use crate::verification::VerificationProvider;
use crate::wallet;
use crate::{
    checked_sats_add, checked_sum, internal_error, not_found, now_timestamp, permission_denied,
    unavailable,
};

/// Longest window one signed request may cover.
///
/// Set to the FI client's own `FI_LIQUIDITY_REQUEST_VALIDITY`, which is one
/// hour. A ceiling below what the shipped consumer asks for would refuse every
/// real request, so this is not a number FLIP gets to pick freely — it is the
/// contract's, and the same hour `FLIP_TRUST_MATERIAL_MAX_VALIDITY_SECS`
/// already uses.
///
/// What matters for the bound is that a limit exists at all: without one a
/// requester chose its own expiry, and one signature stayed deliverable for as
/// long as it liked.
const MAX_REQUEST_LIFETIME_SECS: u64 = 3_600;

/// Clock disagreement tolerated on `issued_at`.
const MAX_ISSUED_AT_SKEW_SECS: u64 = 120;

impl PublicLiquidityApi for DaemonContext {
    async fn get_provider_info(
        &self,
        request: Signed<GetProviderInfoRequest>,
    ) -> ServiceResult<Signed<GetProviderInfoResponse>> {
        // One handle for the whole request: verifying and signing under
        // different providers would let a concurrent identity install split a
        // single exchange across two keys.
        let auth_provider = self.auth_provider().await;
        auth_provider.verify_get_provider_info_request(&request)?;
        let provider_pubkey = identity::load_provider_identity(&self.database).await?;
        if request.payload.provider_pubkey != provider_pubkey {
            return Err(permission_denied("request targets a different provider"));
        }

        let setup = setup_store::load_setup_state(&self.database).await?;
        let readiness = advertisement::public_readiness(self).await?;
        let (supported_sources, policy, endpoint_id, outcome) =
            provider_info_parts(&setup, &request.payload, readiness.ready);
        let response = GetProviderInfoResponse {
            version: PROTOCOL_VERSION,
            provider_pubkey,
            issued_at: now_timestamp(),
            advertisement_hash: request.payload.advertisement_hash,
            supported_sources,
            policy,
            api_endpoint_id: endpoint_id,
            api_version: PROTOCOL_VERSION,
            outcome,
        };

        auth_provider.sign_get_provider_info_response(response)
    }

    async fn request_liquidity(
        &self,
        request: Signed<RequestLiquidityRequest>,
    ) -> ServiceResult<Signed<RequestLiquidityResponse>> {
        let auth_provider = self.auth_provider().await;
        auth_provider.verify_request_liquidity_request(&request)?;
        let provider_pubkey = identity::load_provider_identity(&self.database).await?;
        if request.payload.provider_pubkey != provider_pubkey {
            return Err(permission_denied("request targets a different provider"));
        }

        let setup = setup_store::load_setup_state(&self.database).await?;
        let readiness = advertisement::public_readiness(self).await?;
        accept_or_reject_request(
            RequestDeps {
                database: &self.database,
                auth_provider: auth_provider.as_ref(),
                verification_provider: self.verification_provider.as_ref(),
                allocation_admission: &self.allocation_admission,
            },
            &setup,
            readiness.ready,
            request,
        )
        .await
    }

    async fn get_allocation_status(
        &self,
        request: Signed<GetAllocationStatusRequest>,
    ) -> ServiceResult<Signed<GetAllocationStatusResponse>> {
        let auth_provider = self.auth_provider().await;
        auth_provider.verify_get_allocation_status_request(&request)?;
        let provider_pubkey = identity::load_provider_identity(&self.database).await?;
        if request.payload.provider_pubkey != provider_pubkey {
            return Err(permission_denied("request targets a different provider"));
        }

        let status = allocation_store::load_allocation_status_for_poll(
            &self.database,
            &request.payload.requester_pubkey,
            request.payload.details_payload_hash,
        )
        .await?
        .ok_or_else(|| not_found("allocation status not found"))?;
        let response = GetAllocationStatusResponse {
            version: PROTOCOL_VERSION,
            provider_pubkey,
            issued_at: now_timestamp(),
            status,
        };

        auth_provider.sign_get_allocation_status_response(response)
    }
}

/// The daemon-generation state one liquidity request is decided against.
///
/// Bundled rather than passed one by one so the list can grow with the gates
/// the request has to pass without the signature becoming the thing a reader
/// has to parse.
struct RequestDeps<'a> {
    database: &'a Database,
    auth_provider: &'a dyn PublicAuthProvider,
    verification_provider: &'a dyn VerificationProvider,
    allocation_admission: &'a tokio::sync::RwLock<AllocationAdmission>,
}

/// Decides a liquidity request against current state. Idempotency is
/// semantic: the federation is the allocation's identity, a repeat request is
/// answered from the allocation's current state with a fresh signature, and
/// rejections are stateless -- nothing is persisted, so a retry re-evaluates
/// from scratch (a request rejected for capacity yesterday may be accepted
/// today).
async fn accept_or_reject_request(
    deps: RequestDeps<'_>,
    setup: &StoredSetupState,
    public_ready: bool,
    signed_request: Signed<RequestLiquidityRequest>,
) -> ServiceResult<Signed<RequestLiquidityResponse>> {
    let RequestDeps {
        database,
        auth_provider,
        verification_provider,
        allocation_admission,
    } = deps;
    let request_json = serde_json::to_string(&signed_request).map_err(internal_error)?;
    let request = signed_request.payload;

    // Repeat fast path before validation and verification: an existing
    // allocation answers without re-verifying (verification may do bounded
    // network I/O), even after the original request's expiry has passed.
    if let Some(response) = respond_from_existing_allocation(
        database,
        auth_provider,
        &request,
        ExistingAllocationDisclosure::RequesterOnly,
    )
    .await?
    {
        return Ok(response);
    }

    if let Some((code, reason)) = pre_validation_failure(setup, public_ready, &request)? {
        return stateless_rejection(auth_provider, &request, code, Some(reason));
    }
    let config = setup
        .config
        .as_ref()
        .ok_or_else(|| internal_error("pre-validation passed without setup config"))?;

    // Verification runs before the write transaction opens: it may block on
    // network deadlines and must never hold the SQLite writer.
    let outcome = verification_provider.verify(&request, config).await;
    if let Some(rejection) = outcome.rejection {
        return stateless_rejection(auth_provider, &request, rejection.code, rejection.reason);
    }

    // The restore path takes the write side after staging, then compares every
    // allocation committed before this point with the staged archive. Holding
    // this guard through commit closes the otherwise possible race where restore
    // validates an archive and a concurrent request commits just before teardown.
    let allocation_admission = allocation_admission.read().await;
    if !allocation_admission.accepts_new_allocation() {
        return Err(unavailable(
            "new allocation acceptance is closed for live restore",
        ));
    }

    let mut tx = database.begin_write().await.map_err(internal_error)?;

    // The setup snapshot that admitted this request must still be current.
    //
    // `request_liquidity` loads `StoredSetupState`, verification awaits outside
    // this transaction, and `plan_allocation` then uses that detached snapshot.
    // `apply_setup_config`
    // and `update_provider_config` increment `setup_state.revision` atomically,
    // so an Admin update that removes the trust, policy, or source which admitted
    // this request could commit while its verifier was still in flight — and the
    // allocation would commit afterwards, authorized by a snapshot that no longer
    // exists.
    //
    // Read inside the write transaction, so it fences the commit rather than
    // adding another racy check before it.
    let current_revision = crate::setup_store::setup_revision_tx(&mut tx).await?;
    if current_revision != setup.revision {
        tx.rollback().await.map_err(internal_error)?;
        return stateless_rejection(
            auth_provider,
            &request,
            PublicRejectionCode::ProviderUnavailable,
            Some(
                "provider setup changed while this request was being verified; \
                 request again against the current terms"
                    .to_owned(),
            ),
        );
    }

    // Expiry is rechecked here, and not only in `pre_validation_failure`.
    //
    // Verification runs outbound network work between the two checks and may
    // outlast the request that triggered it. Someone choosing a request valid
    // for one second and delaying a verification dependency past it would
    // otherwise have FLIP commit an allocation and sign `accepted` for a request
    // that had already expired.
    //
    // Inside the write transaction rather than before it: that ties the check to
    // the commit it guards instead of to some earlier moment, so nothing can pass
    // the fence and then wait for the writer. Repeats of an allocation that already
    // exists never reach here — they answered from the fast path above — so this
    // cannot break the idempotency `SPEC-flip-rpc` requires.
    if request.expires_at.0 <= now_timestamp().0 {
        tx.rollback().await.map_err(internal_error)?;
        return stateless_rejection(
            auth_provider,
            &request,
            PublicRejectionCode::RequestExpired,
            Some("request expired while its trust material was being verified".to_owned()),
        );
    }

    let Some(plan) = plan_allocation(&mut tx, config, &request).await? else {
        tx.rollback().await.map_err(internal_error)?;
        return stateless_rejection(
            auth_provider,
            &request,
            PublicRejectionCode::InsufficientCapacity,
            Some("wallet-backed capacity is insufficient for this request".to_owned()),
        );
    };
    let mut inserted = allocation_store::insert_allocation(
        &mut tx,
        &request,
        &request_json,
        &outcome.summary,
        plan.committed_amount,
        plan.reserved_amount,
        &plan.items,
    )
    .await?;

    // The federation already has an allocation held by someone else. If it
    // holds nothing, release it and take the federation.
    //
    // `SPEC-flip-rpc` rules this. One
    // allocation per federation is what stops a published endorsement being
    // drawn down repeatedly, but it also decided *who* held that allocation,
    // and with one `INSERT OR IGNORE` and no production `UPDATE` or `DELETE`
    // that decision was permanent. An idle allocation excluded an
    // equally-credentialed requester from a federation forever while holding
    // nothing of value. Once real work starts it locks again, which is the
    // state that actually matters.
    //
    // Inside the same transaction as the insert it feeds, so releasing the idle
    // allocation and taking it are one atomic step that no third caller can
    // split.
    if !inserted && take_over_idle_allocation(&mut tx, &request).await? {
        inserted = allocation_store::insert_allocation(
            &mut tx,
            &request,
            &request_json,
            &outcome.summary,
            plan.committed_amount,
            plan.reserved_amount,
            &plan.items,
        )
        .await?;
    }

    if !inserted {
        // Lost a concurrent race for the same federation, or the existing
        // allocation still holds work: the incumbent's allocation answers this
        // request too.
        tx.rollback().await.map_err(internal_error)?;
        return respond_from_existing_allocation(
            database,
            auth_provider,
            &request,
            ExistingAllocationDisclosure::Verified,
        )
        .await?
        .ok_or_else(|| internal_error("allocation insert was ignored but no allocation exists"));
    }
    tx.commit().await.map_err(internal_error)?;
    tracing::info!(
        federation_id = %request.federation_details.federation_id.0,
        requester_pubkey = %request.requester_pubkey.0,
        items = plan.items.len(),
        committed_sats = plan.committed_amount.0,
        reserved_sats = plan.reserved_amount.0,
        "accepted liquidity request"
    );
    accepted_response(auth_provider, &request, plan.initial_status)
}

/// Answers a request whose federation already has an allocation, or returns
/// `None` when it has none. A repeat with the same details commitment from
/// the original requester gets the allocation's current status, freshly
/// signed; different details for the same federation are a conflict, because
/// a federation only ever has one allocation; a matching commitment under a
/// different requester key is never served and returns `None` to fall
/// through to full validation.
/// What a caller has already established about its right to be told that a
/// federation has an allocation at all.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ExistingAllocationDisclosure {
    /// Nothing yet. Reached before validation and verification, by anyone who
    /// can sign a request under a key of their own choosing — which is every
    /// caller, since public authentication verifies the signature against the
    /// key the request declares.
    ///
    /// Only the allocation's own requester learns anything here.
    RequesterOnly,

    /// The caller passed full verification for this federation, which means it
    /// holds a valid unrevoked FMan endorsement for it. Naming the conflict is
    /// then the answer to the question it actually asked.
    Verified,
}

async fn respond_from_existing_allocation(
    database: &Database,
    auth_provider: &dyn PublicAuthProvider,
    request: &RequestLiquidityRequest,
    disclosure: ExistingAllocationDisclosure,
) -> ServiceResult<Option<Signed<RequestLiquidityResponse>>> {
    let federation_id = &request.federation_details.federation_id;
    let row = sqlx::query(
        "SELECT requester_pubkey, details_payload_hash FROM allocations WHERE federation_id = ?",
    )
    .bind(&federation_id.0)
    .fetch_optional(database.pool())
    .await
    .map_err(internal_error)?;
    let Some(row) = row else {
        return Ok(None);
    };

    // The details commitment covers the requester key, but this path can run
    // before that hash is checked against the canonical request details, so a
    // matching hash may simply have been copied. The stored requester key is
    // what binds a caller to this allocation, and it is checked first.
    let is_requester = row.get::<String, _>("requester_pubkey") == request.requester_pubkey.0;
    let details_match =
        row.get::<Vec<u8>, _>("details_payload_hash") == request.details_payload_hash.0;

    let conflict = |reason: String| {
        stateless_rejection(
            auth_provider,
            request,
            PublicRejectionCode::RequestConflict,
            Some(reason),
        )
        .map(Some)
    };

    match (is_requester, details_match, disclosure) {
        // Same requester, same details: the repeat this fast path exists for.
        (true, true, _) => {
            let status =
                allocation_store::load_allocation_status_by_federation(database, federation_id)
                    .await?
                    .ok_or_else(|| {
                        internal_error("allocation row exists without a loadable status")
                    })?;
            accepted_response(auth_provider, request, status).map(Some)
        }

        // Same requester, different details. A federation has one allocation,
        // so this is a genuine conflict and its own requester may be told so.
        (true, false, _) => conflict(format!(
            "federation {} already has an allocation with different request details",
            federation_id.0
        )),

        // Anyone else, with nothing yet establishing a right to know this
        // federation has an allocation. Answering here — with `request_conflict`
        // for a wrong hash, or `accepted` for a copied one — would make one
        // signature under a self-chosen key into an existence oracle over
        // arbitrary federation ids. Falling through instead makes the response
        // identical to the one a federation with no allocation produces,
        // because nothing between here and verification reads `allocations`.
        (false, _, ExistingAllocationDisclosure::RequesterOnly) => Ok(None),

        // Anyone else, having passed verification for this federation. They
        // hold an endorsement for it, so its allocation's existence is not
        // something they are being told for the first time.
        (false, _, ExistingAllocationDisclosure::Verified) => conflict(format!(
            "federation {} already has an allocation for a different requester",
            federation_id.0
        )),
    }
}

/// Releases this federation's allocation when another requester holds it and it
/// holds nothing, so the caller's own insert can take the federation.
///
/// Reached only after full verification, so the caller holds a valid unrevoked
/// FMan endorsement for this federation. That is the same standing
/// [`ExistingAllocationDisclosure::Verified`] already requires before a caller
/// is told the allocation exists at all, so acting on it discloses nothing the
/// `request_conflict` answer did not. **Do not reach this from the
/// `RequesterOnly` path**, which exists precisely so that an unverified caller
/// cannot use this table as an existence oracle over arbitrary federation ids.
///
/// Only a *different* requester takes over. The same requester arriving with
/// different details is the genuine conflict `respond_from_existing_allocation`
/// names, and silently replacing their own allocation is not what they asked
/// for.
async fn take_over_idle_allocation(
    tx: &mut Transaction<'_, Sqlite>,
    request: &RequestLiquidityRequest,
) -> ServiceResult<bool> {
    let federation_id = &request.federation_details.federation_id;
    let current: Option<String> =
        sqlx::query_scalar("SELECT requester_pubkey FROM allocations WHERE federation_id = ?")
            .bind(&federation_id.0)
            .fetch_optional(&mut **tx)
            .await
            .map_err(internal_error)?;
    // No row means the insert was ignored for some other reason and the caller
    // should take the conflict path rather than this one.
    let Some(current) = current else {
        return Ok(false);
    };
    if current == request.requester_pubkey.0 {
        return Ok(false);
    }

    match allocation_store::release_allocation_tx(tx, federation_id).await? {
        allocation_store::AllocationReleaseOutcome::Released { previous_requester } => {
            let detail_json = serde_json::json!({
                "federation_id": federation_id,
                "previous_requester": previous_requester,
                "new_requester": request.requester_pubkey,
                "detail": "idle allocation taken over by a verified requester",
            });
            sqlx::query(
                "INSERT INTO audit_log (action, detail_json, created_at) \
                 VALUES (?, ?, unixepoch())",
            )
            .bind("take_over_idle_allocation")
            .bind(detail_json.to_string())
            .execute(&mut **tx)
            .await
            .map_err(internal_error)?;
            tracing::info!(
                federation_id = %federation_id.0,
                previous_requester = %previous_requester.0,
                new_requester = %request.requester_pubkey.0,
                "released an idle allocation for a verified requester",
            );
            Ok(true)
        }
        allocation_store::AllocationReleaseOutcome::Held(holding) => {
            tracing::debug!(
                federation_id = %federation_id.0,
                reserving_items = holding.reserving_items,
                pending_operations = holding.pending_operations,
                fulfilled_sats = holding.fulfilled_sats,
                "refused a takeover: the allocation still holds work or value",
            );
            Ok(false)
        }
        allocation_store::AllocationReleaseOutcome::NotFound => Ok(false),
    }
}

/// Rejections are stateless: the response is signed and returned, nothing is
/// persisted, and this log line is the only provider-side record.
fn stateless_rejection(
    auth_provider: &dyn PublicAuthProvider,
    request: &RequestLiquidityRequest,
    code: PublicRejectionCode,
    reason: Option<String>,
) -> ServiceResult<Signed<RequestLiquidityResponse>> {
    tracing::info!(
        federation_id = %request.federation_details.federation_id.0,
        requester_pubkey = %request.requester_pubkey.0,
        code = %code,
        reason = reason.as_deref().unwrap_or(""),
        "rejected liquidity request",
    );
    rejected_response(auth_provider, request, code, reason)
}

/// Pure request pre-checks that run before verification and outside any
/// database transaction. Returns the first failing rejection, if any.
fn pre_validation_failure(
    setup: &StoredSetupState,
    public_ready: bool,
    request: &RequestLiquidityRequest,
) -> ServiceResult<Option<(PublicRejectionCode, String)>> {
    if !public_ready {
        return Ok(Some((
            PublicRejectionCode::ProviderUnavailable,
            "provider public endpoint is not ready".to_owned(),
        )));
    }

    let Some(config) = setup
        .config
        .as_ref()
        .filter(|_| setup.status == SetupStatus::Ready)
    else {
        return Ok(Some((
            PublicRejectionCode::ProviderUnavailable,
            "setup is not ready".to_owned(),
        )));
    };

    if request.version != PROTOCOL_VERSION {
        return Ok(Some((
            PublicRejectionCode::VersionUnsupported,
            "Public Liquidity API version is unsupported".to_owned(),
        )));
    }
    let now = now_timestamp().0;
    if request.expires_at.0 <= now {
        return Ok(Some((
            PublicRejectionCode::RequestExpired,
            "request is expired".to_owned(),
        )));
    }
    // A signature is only as reusable as the window it commits to. Without a
    // ceiling, a requester chose its own expiry — years out if it liked — and
    // one signed request stayed deliverable for that whole window, so every
    // cost of evaluating it could be incurred again from a single signature.
    // A bounded window therefore means a fresh signature per window.
    //
    // What binds both timestamps is the outer public-RPC signature over the
    // whole payload, not the details commitment:
    // `RequestLiquidityDetailsCommitmentV1` carries `expires_at` and not
    // `issued_at`.
    if request.expires_at.0.saturating_sub(request.issued_at.0) > MAX_REQUEST_LIFETIME_SECS {
        return Ok(Some((
            PublicRejectionCode::RequestExpired,
            format!("request lifetime exceeds {MAX_REQUEST_LIFETIME_SECS} seconds"),
        )));
    }
    // A future `issued_at` would otherwise buy the same unbounded window back
    // by pairing it with an expiry the same distance further out.
    if request.issued_at.0 > now.saturating_add(MAX_ISSUED_AT_SKEW_SECS) {
        return Ok(Some((
            PublicRejectionCode::RequestExpired,
            "request issued_at is too far in the future".to_owned(),
        )));
    }
    if request.network != config.network
        || !config.policy.supported_networks.contains(&request.network)
    {
        return Ok(Some((
            PublicRejectionCode::UnsupportedNetwork,
            "request network is not supported".to_owned(),
        )));
    }

    if let Err(reason) = validate_amount_bounds(&request.amounts) {
        return Ok(Some((PublicRejectionCode::InvalidAmountBounds, reason)));
    }
    if let Err(reason) =
        validate_supported_sources(&request.amounts, &config.capacity.supported_sources)
    {
        return Ok(Some((PublicRejectionCode::UnsupportedSourceType, reason)));
    }
    let expected_details_hash =
        request_liquidity_details_hash_for_request(request).map_err(internal_error)?;
    if expected_details_hash != request.details_payload_hash {
        return Ok(Some((
            PublicRejectionCode::InvalidDetailsPayload,
            "details_payload_hash does not match canonical request details".to_owned(),
        )));
    }

    Ok(None)
}

async fn plan_allocation(
    tx: &mut Transaction<'_, Sqlite>,
    config: &SetupConfigView,
    request: &RequestLiquidityRequest,
) -> ServiceResult<Option<AcceptedPlan>> {
    let mut items = Vec::new();
    if request.amounts.gateway_min_amount.0 > 0 {
        let amount = request.amounts.gateway_min_amount;
        let reserved_amount = checked_sats_add(amount, config.funding_policy.fee_reserve)?;
        let item_id = allocation_store::item_id(
            &request.federation_details.federation_id,
            SourceType::Gateway,
        );
        items.push(PlannedItem {
            item_id: item_id.clone(),
            source_type: SourceType::Gateway,
            target: AllocationItemTarget::Gateway {
                item_id,
                gateway_id: config.gateway.gateway_id.clone(),
                gateway_name: config.gateway.gateway_name.clone(),
                amount,
            },
            amount,
            reserved_amount,
        });
    }
    if request.amounts.stability_min_amount.0 > 0 {
        let amount = request.amounts.stability_min_amount;
        let reserved_amount = checked_sats_add(amount, config.funding_policy.fee_reserve)?;
        let item_id = allocation_store::item_id(
            &request.federation_details.federation_id,
            SourceType::StabilityPool,
        );
        items.push(PlannedItem {
            item_id: item_id.clone(),
            source_type: SourceType::StabilityPool,
            target: AllocationItemTarget::StabilityPool { item_id, amount },
            amount,
            reserved_amount,
        });
    }

    let committed_amount = checked_sum(items.iter().map(|item| item.amount))?;
    let reserved_amount = checked_sum(items.iter().map(|item| item.reserved_amount))?;
    let Some(balance) = wallet::latest_wallet_balance_observation_tx(tx).await? else {
        return Ok(None);
    };
    if balance.network != config.network.to_string() {
        return Ok(None);
    }
    let available_wallet_balance =
        funds_admin::available_balance_for_request(tx, config, balance.spendable).await?;
    let capacity_basis = match config.capacity.mode {
        CapacityMode::AvailableFunds => available_wallet_balance.0,
        CapacityMode::ExplicitCap => {
            let Some(explicit_cap) = config.capacity.explicit_cap else {
                return Ok(None);
            };
            let active_reserved_amount = wallet::active_reserved_amount_tx(tx).await?;
            let explicit_remaining = explicit_cap.0.saturating_sub(active_reserved_amount.0);
            explicit_remaining.min(available_wallet_balance.0)
        }
    };
    if reserved_amount.0 > capacity_basis {
        return Ok(None);
    }

    let now = now_timestamp();
    let initial_status = AllocationStatus {
        details_payload_hash: request.details_payload_hash,
        provider_pubkey: request.provider_pubkey.clone(),
        item_statuses: items
            .iter()
            .map(|item| AllocationItemStatus {
                target: item.target.clone(),
                status: ItemAllocationStatus::Pending,
                fulfilled_amount: None,
                completion_evidence: None,
                failure: None,
                updated_at: now,
            })
            .collect(),
    };

    Ok(Some(AcceptedPlan {
        committed_amount,
        reserved_amount,
        items,
        initial_status,
    }))
}

fn provider_info_parts(
    setup: &StoredSetupState,
    request: &GetProviderInfoRequest,
    public_ready: bool,
) -> (
    Vec<SourceType>,
    ProviderPolicy,
    RpcEndpointId,
    ProviderInfoOutcome,
) {
    let version_supported = request.version == PROTOCOL_VERSION
        && request
            .client_supported_versions
            .contains(&PROTOCOL_VERSION);
    let Some(config) = setup
        .config
        .as_ref()
        .filter(|_| setup.status == SetupStatus::Ready && public_ready)
    else {
        return (
            Vec::new(),
            empty_policy(),
            RpcEndpointId("unconfigured".to_owned()),
            ProviderInfoOutcome::Rejected(PublicRejection {
                code: if version_supported {
                    PublicRejectionCode::ProviderUnavailable
                } else {
                    PublicRejectionCode::VersionUnsupported
                },
                reason: None,
            }),
        );
    };

    let endpoint_id = config
        .advertised_endpoint
        .endpoint_id
        .clone()
        .unwrap_or_else(|| RpcEndpointId("default".to_owned()));
    let outcome = if version_supported {
        ProviderInfoOutcome::Available
    } else {
        ProviderInfoOutcome::Rejected(PublicRejection {
            code: PublicRejectionCode::VersionUnsupported,
            reason: None,
        })
    };

    (
        config.capacity.supported_sources.clone(),
        config.policy.clone(),
        endpoint_id,
        outcome,
    )
}

fn validate_amount_bounds(amounts: &LiquidityAmountBounds) -> Result<(), String> {
    let gateway_requested = amounts.gateway_min_amount.0 > 0;
    let stability_requested = amounts.stability_min_amount.0 > 0;
    if !gateway_requested && !stability_requested {
        return Err("at least one source minimum must be non-zero".to_owned());
    }
    if !gateway_requested && amounts.gateway_max_amount.is_some() {
        return Err("gateway max must be absent when gateway is not requested".to_owned());
    }
    if !stability_requested && amounts.stability_max_amount.is_some() {
        return Err("stability max must be absent when stability is not requested".to_owned());
    }
    if let Some(max) = amounts.gateway_max_amount
        && max.0 < amounts.gateway_min_amount.0
    {
        return Err("gateway max must be greater than or equal to gateway min".to_owned());
    }
    if let Some(max) = amounts.stability_max_amount
        && max.0 < amounts.stability_min_amount.0
    {
        return Err("stability max must be greater than or equal to stability min".to_owned());
    }
    Ok(())
}

fn validate_supported_sources(
    amounts: &LiquidityAmountBounds,
    supported_sources: &[SourceType],
) -> Result<(), String> {
    if amounts.gateway_min_amount.0 > 0 && !supported_sources.contains(&SourceType::Gateway) {
        return Err("gateway source type is not supported".to_owned());
    }
    if amounts.stability_min_amount.0 > 0 && !supported_sources.contains(&SourceType::StabilityPool)
    {
        return Err("stability_pool source type is not supported".to_owned());
    }
    Ok(())
}

fn accepted_response(
    auth_provider: &dyn PublicAuthProvider,
    request: &RequestLiquidityRequest,
    status: AllocationStatus,
) -> ServiceResult<Signed<RequestLiquidityResponse>> {
    response(
        auth_provider,
        request,
        RequestLiquidityOutcome::Accepted(status),
        now_timestamp(),
    )
}

fn rejected_response(
    auth_provider: &dyn PublicAuthProvider,
    request: &RequestLiquidityRequest,
    code: PublicRejectionCode,
    reason: Option<String>,
) -> ServiceResult<Signed<RequestLiquidityResponse>> {
    response(
        auth_provider,
        request,
        RequestLiquidityOutcome::Rejected(PublicRejection { code, reason }),
        now_timestamp(),
    )
}

fn response(
    auth_provider: &dyn PublicAuthProvider,
    request: &RequestLiquidityRequest,
    outcome: RequestLiquidityOutcome,
    issued_at: Timestamp,
) -> ServiceResult<Signed<RequestLiquidityResponse>> {
    auth_provider.sign_request_liquidity_response(RequestLiquidityResponse {
        version: PROTOCOL_VERSION,
        details_payload_hash: request.details_payload_hash,
        provider_pubkey: request.provider_pubkey.clone(),
        issued_at,
        outcome,
    })
}

fn empty_policy() -> ProviderPolicy {
    ProviderPolicy {
        accepted_attester_policies: Vec::<AcceptedAttesterPolicy>::new(),
        supported_networks: Vec::new(),
    }
}

struct AcceptedPlan {
    committed_amount: Sats,
    reserved_amount: Sats,
    items: Vec<PlannedItem>,
    initial_status: AllocationStatus,
}

/// Serves the app-facing Public Liquidity API over Iroh until daemon shutdown.
///
/// The transport identity is derived from the provider signing identity, so the
/// advertised node id is stable across restarts. That has two consequences for
/// this function: it cannot bind before an identity exists, and once it binds
/// it can settle the advertised endpoint address itself.
pub(crate) async fn serve(context: DaemonContext) -> anyhow::Result<()> {
    let Some(secret_key) = wait_for_transport_identity(&context).await? else {
        // Shutdown won the race against an identity ever being installed.
        return Ok(());
    };

    let endpoint = if std::env::var_os("DEV_DEFE_SOCKET_PATH").is_some() {
        Endpoint::builder(presets::N0DisableRelay)
            .secret_key(secret_key)
            .clear_ip_transports()
            .bind_addr(context.args.public_bind_address)
            .context("failed to configure Public Liquidity API Iroh bind address")?
            .bind()
            .await
    } else {
        Endpoint::builder(presets::N0)
            .secret_key(secret_key)
            .clear_ip_transports()
            .bind_addr(context.args.public_bind_address)
            .context("failed to configure Public Liquidity API Iroh bind address")?
            .bind()
            .await
    }
    .context("failed to bind Public Liquidity API Iroh endpoint")?;
    let router = Router::builder(endpoint)
        .accept(
            PUBLIC_LIQUIDITY_API_ALPN,
            IrohProtocol::new(PublicLiquidityApiServer::new(context.clone())),
        )
        .spawn();
    // Binding is sufficient for direct transports. Iroh establishes public
    // relay connectivity asynchronously; waiting for it here would make daemon
    // startup and shutdown depend on external DNS/network availability.
    let endpoint_addr = router.endpoint().addr();
    let node_id = router.endpoint().id().to_string();
    {
        let mut state = context.daemon_state.write().await;
        state.public_iroh_node_id = Some(node_id.clone());
    }
    tracing::info!(
        ?endpoint_addr,
        %node_id,
        "Public Liquidity API Iroh transport listening"
    );
    write_public_endpoint_addr(&context, &endpoint_addr).await?;
    // Best-effort: a deployment with no setup config yet has nothing to adopt,
    // and a database error here must not take down the transport that just came
    // up successfully.
    if let Err(error) =
        setup_store::adopt_local_iroh_endpoint_address(&context.database, &node_id).await
    {
        tracing::warn!(
            ?error,
            "failed to adopt the local Iroh node id as the advertised endpoint address"
        );
    }
    // The endpoint identity is a readiness input, so do not let the publisher
    // race this bind and withdraw a persisted advertisement while the node id
    // is still unavailable. Its first pass reconciles immediately.
    context
        .background_tasks
        .spawn(advertisement::run_publisher_task(context.clone()));

    context.shutdown.clone().cancelled_owned().await;
    router
        .shutdown()
        .await
        .context("Public Liquidity API Iroh router shutdown failed")?;
    Ok(())
}

/// Resolves the derived Iroh transport key, waiting for a provider identity to
/// be installed if the daemon booted without one.
///
/// Returns `None` when shutdown arrives first. Deferring costs nothing: without
/// a provider identity every public call fails closed on signing anyway, and
/// binding early would only publish a node id that the installed identity would
/// then contradict.
async fn wait_for_transport_identity(context: &DaemonContext) -> anyhow::Result<Option<SecretKey>> {
    let mut installed = context.identity_installed.subscribe();
    loop {
        if *installed.borrow_and_update() {
            break;
        }
        tracing::info!(
            "Public Liquidity API transport is waiting for a provider signing identity; \
             install one through the Admin API"
        );
        tokio::select! {
            _ = context.shutdown.cancelled() => return Ok(None),
            result = installed.changed() => {
                result.context("provider identity signal closed")?;
            }
        }
    }

    let identity =
        identity::load_production_provider_identity(&context.database, &context.secret_store)
            .await?
            .context("provider identity signalled as installed but is not readable")?;
    Ok(Some(identity.derive_iroh_secret_key()?))
}

const PUBLIC_ENDPOINT_ADDR_FILE: &str = "public-iroh-endpoint-addr.json";
pub(crate) const PUBLIC_ENDPOINT_ADDR_TEMP_FILE: &str = "public-iroh-endpoint-addr.json.tmp";

/// Persist the resolved public endpoint so local operators and clients can
/// discover direct transport addresses selected during Iroh startup.
async fn write_public_endpoint_addr(
    context: &DaemonContext,
    endpoint_addr: &fedi_iroh_rpc::iroh::EndpointAddr,
) -> anyhow::Result<()> {
    let path = context.paths.data_dir.join(PUBLIC_ENDPOINT_ADDR_FILE);
    let json = serde_json::to_vec_pretty(endpoint_addr)
        .context("serialize Public Liquidity API endpoint address")?;
    write_public_endpoint_addr_file(&path, &json).await
}

/// Atomically replace the published endpoint address without exposing a
/// truncated or partially written destination to concurrent readers.
async fn write_public_endpoint_addr_file(
    path: &std::path::Path,
    json: &[u8],
) -> anyhow::Result<()> {
    let temporary_path = path.with_file_name(PUBLIC_ENDPOINT_ADDR_TEMP_FILE);
    tokio::fs::write(&temporary_path, json)
        .await
        .with_context(|| {
            format!(
                "write temporary public endpoint address to {}",
                temporary_path.display()
            )
        })?;
    if let Err(error) = tokio::fs::rename(&temporary_path, path).await {
        let _ = tokio::fs::remove_file(&temporary_path).await;
        return Err(error)
            .with_context(|| format!("publish public endpoint address to {}", path.display()));
    }
    Ok(())
}

#[cfg(test)]
#[path = "../tests/public.rs"]
mod tests;
