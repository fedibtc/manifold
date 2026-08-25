//! Deployment setup: the stored config, its validation, hot reload, and the
//! audit log.
//!
//! The config decides which gateway an accepted allocation pays and which
//! federations FLIP will serve, so its hard fields are fixed at first setup and
//! every write is fenced on the revision it read. Secrets are written by name,
//! one at a time, so a config write can neither store nor remove one.

use std::time::Duration;

use fedi_decentralized_service_liquidity_manager::{
    AdvertisementConfig, ApplySetupConfigRequest, ApplySetupConfigResponse, AttestationSummary,
    BitcoinNetwork, ChainObserverBackend, ChainObserverBackendView, ChainObserverConfig,
    ChainObserverConfigView, ConfigSecret, FundingPolicyConfig, GatewayConfigView,
    GetProviderConfigResponse, GetSetupStateResponse, ProbeGatewayRequest, ProbeGatewayResponse,
    ProviderConfigPatch, ProviderDisplayPatch, ProviderPolicy, RpcEndpointAddress, RpcTransport,
    SecretString, SecretUpdate, ServiceError, ServiceResult, SetConfigSecretRequest,
    SetConfigSecretResponse, SetupConfig, SetupConfigView, SetupStatus, SetupValidationCheck,
    SetupValidationSummary, UpdateProviderConfigRequest, UpdateProviderConfigResponse,
    ValidateSetupRequest, ValidateSetupResponse, ValidationStatus, WalletOperationId,
};
use sqlx::{Row, Sqlite, Transaction};
use tokio::net::TcpStream;
use tokio::time::timeout;

use crate::chain_observer::{ChainObserver, ConfiguredChainObserver};
use crate::database::Database;
use crate::secret_store::{self, EncryptedSecretRecord, SecretStore};
use crate::wallet::{FundsWallet, GatewaydFundsWallet};
use crate::{failed_precondition, internal_error, invalid_argument};

const SETUP_STATE_ID: i64 = 1;
const GATEWAY_ADMIN_SECRET: &str = "gateway.admin_credential";
const BITCOIND_PASSWORD_SECRET: &str = "chain_observer.bitcoind.password";
const TCP_REACHABILITY_TIMEOUT: Duration = Duration::from_millis(500);
const GATEWAY_API_VALIDATION_TIMEOUT: Duration = Duration::from_secs(15);
const CHAIN_OBSERVER_API_VALIDATION_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_STABILITY_POOL_FEE_RATE_PPB: u64 = 1_000_000_000;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct StoredSetupState {
    pub status: SetupStatus,
    pub config: Option<SetupConfigView>,
    pub validation: Option<SetupValidationSummary>,

    /// Which revision of the setup row this snapshot was read from.
    ///
    /// Every write of the row increments it, because every write goes through
    /// [`upsert_setup_state_tx`]. Pass it back to that function to fence the
    /// write against an update that landed since this snapshot was taken. A
    /// caller that instead carries the snapshot across an await without writing
    /// can compare this against [`setup_revision_tx`] to find out whether an
    /// Admin update superseded what it is still acting on.
    pub revision: i64,
}

pub(crate) async fn get_setup_state(database: &Database) -> ServiceResult<GetSetupStateResponse> {
    let stored = load_setup_state(database).await?;
    let missing_fields = missing_fields(&stored);
    Ok(GetSetupStateResponse {
        status: stored.status,
        config: stored.config,
        missing_fields,
        validation: stored.validation,
    })
}

pub(crate) async fn get_provider_config(
    database: &Database,
) -> ServiceResult<GetProviderConfigResponse> {
    let stored = load_setup_state(database).await?;
    let config = stored
        .config
        .ok_or_else(|| failed_precondition("setup config is not configured"))?;
    Ok(GetProviderConfigResponse { config })
}

/// Rejects fixture-mode/network combinations that must never reach persisted
/// config: `--trust-fixtures` substitutes the federation preview and FMan
/// trust-material verification inputs with local files, so a fixture-fed
/// daemon must never operate against Bitcoin mainnet.
pub(crate) fn ensure_trust_fixtures_allow_network(
    trust_fixtures_enabled: bool,
    network: BitcoinNetwork,
) -> ServiceResult<()> {
    if trust_fixtures_enabled && network == BitcoinNetwork::Bitcoin {
        return Err(invalid_argument(
            "trust fixtures cannot be used with the bitcoin mainnet network; \
             restart the daemon without --trust-fixtures (FLIP_TRUST_FIXTURES)",
        ));
    }
    Ok(())
}

/// Rejects the loopback/private endpoint allowance on mainnet.
///
/// The allowance exists so local harnesses can reach a federation on
/// `127.0.0.1`. On mainnet it turns off the only guard standing between a
/// valid endorsement and a dial to whatever address a requester names, so a
/// deployment carrying real value must not have it.
///
/// This is a boot backstop rather than a check inside the policy, and it runs
/// per runtime generation so a live restore cannot bring in a mainnet config
/// under a flag a restart would have refused. It does not fire the moment an
/// operator applies a mainnet config to an already-running daemon that was
/// started with the flag — that gap closes at the next generation.
pub(crate) fn ensure_private_endpoints_allow_network(
    allow_private_federation_endpoints: bool,
    network: BitcoinNetwork,
) -> ServiceResult<()> {
    if allow_private_federation_endpoints && network == BitcoinNetwork::Bitcoin {
        return Err(invalid_argument(
            "loopback and private federation endpoints cannot be allowed on the bitcoin \
             mainnet network; restart the daemon without \
             --allow-private-federation-endpoints \
             (FLIP_ALLOW_PRIVATE_FEDERATION_ENDPOINTS)",
        ));
    }
    Ok(())
}

/// Refuses a gateway identity that differs from the one already configured.
///
/// `gateway_id` and `admin_url` decide **where provider money goes**:
/// `GatewaydFundsWallet` builds its base URL from `admin_url` on every worker
/// pass, and the gateway id is what completion evidence attributes the outflow
/// to. Both were replaceable at any time through this verb, including between an
/// item's admission and its send, and the only thing a worker compared
/// afterwards was `network` — so a withdrawal could leave a different gateway
/// wallet than the one the allocation was accepted against.
///
/// So the identity is set once and then frozen. The rest of the gateway stays
/// editable, and that is deliberate: `gateway_name` and `identity_metadata` are
/// display, and `admin_credential` authenticates to the *same* gateway rather
/// than choosing a different one, so freezing it would block credential
/// rotation without protecting anything.
fn ensure_gateway_identity_unchanged(
    stored: Option<&SetupConfigView>,
    candidate: &SetupConfig,
) -> ServiceResult<()> {
    let Some(stored) = stored else {
        return Ok(());
    };
    let candidate_id = candidate.gateway.gateway_id.as_ref();
    if let Some(candidate_id) = candidate_id
        && *candidate_id != stored.gateway.gateway_id
    {
        return Err(failed_precondition(format!(
            "gateway_id is fixed at first setup and cannot be changed from {} to {}; \
             it decides which gateway an accepted allocation pays",
            stored.gateway.gateway_id.0, candidate_id.0
        )));
    }
    if candidate.gateway.admin_url != stored.gateway.admin_url {
        return Err(failed_precondition(format!(
            "gateway admin_url is fixed at first setup and cannot be changed from {} to {}; \
             it decides which wallet an accepted allocation pays",
            stored.gateway.admin_url, candidate.gateway.admin_url
        )));
    }
    Ok(())
}

/// Refuses a funding-policy change while any accepted work could still act on it.
///
/// Later effects of an accepted item must either use acceptance-time inputs or
/// the update must be rejected until the affected work terminates. This is the
/// second route, which needs no per-item snapshot.
///
/// "Could still act on it" is deliberately wider than the item statuses.
/// `confirmations` and `in_doubt_review_after_secs` are read by the wallet sync
/// path, which keeps working on an operation whose item has already gone
/// terminal, so a pending-settlement operation counts as in flight too.
async fn ensure_funding_policy_settled(
    database: &Database,
    stored: Option<&SetupConfigView>,
    candidate: &FundingPolicyConfig,
) -> ServiceResult<()> {
    if !funding_policy_changes(stored, candidate) {
        return Ok(());
    }
    let mut conn = database.pool().acquire().await.map_err(internal_error)?;
    refuse_unsettled_work(unsettled_work_counts(&mut conn).await?)
}

/// The same guard, re-run inside the transaction that persists the config.
///
/// The pool-backed check above is a fast refusal: it runs before the reachability
/// probes so an operator hears "no" immediately rather than after a network round
/// trip. It cannot be the only check. The counts it reads and the write it guards
/// are separated by `normalize_advertised_endpoint` and `validate_candidate_config`
/// — network calls, so seconds — and nothing else covers a request admitted in
/// that window. Not this guard, whose counts predate it; and not the admission
/// path's setup-revision fence, which refuses an allocation whose revision
/// changed *before* it commits, while here the config change commits after.
///
/// `begin_write` opens `BEGIN IMMEDIATE`, so this re-read sees every allocation
/// committed before it and blocks any that would commit after. That is what orders
/// the two writes against each other: whichever loses is refused, by this guard or
/// by the revision fence.
async fn ensure_funding_policy_settled_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    stored: Option<&SetupConfigView>,
    candidate: &FundingPolicyConfig,
) -> ServiceResult<()> {
    if !funding_policy_changes(stored, candidate) {
        return Ok(());
    }
    refuse_unsettled_work(unsettled_work_counts(&mut **tx).await?)
}

fn funding_policy_changes(
    stored: Option<&SetupConfigView>,
    candidate: &FundingPolicyConfig,
) -> bool {
    stored.is_some_and(|stored| stored.funding_policy != *candidate)
}

/// Reserving allocation items and wallet operations still awaiting settlement.
async fn unsettled_work_counts(conn: &mut sqlx::SqliteConnection) -> ServiceResult<(i64, i64)> {
    let mut builder = sqlx::QueryBuilder::new("SELECT COUNT(*) FROM allocation_items WHERE ");
    crate::database::push_in_list(
        &mut builder,
        "status",
        &crate::allocation_store::RESERVING_ITEM_STATUSES,
    );
    let items: i64 = builder
        .build_query_scalar()
        .fetch_one(&mut *conn)
        .await
        .map_err(internal_error)?;

    let mut builder = sqlx::QueryBuilder::new("SELECT COUNT(*) FROM wallet_operations WHERE ");
    crate::database::push_in_list(
        &mut builder,
        "status",
        crate::wallet::PENDING_SETTLEMENT_STATUSES,
    );
    let operations: i64 = builder
        .build_query_scalar()
        .fetch_one(&mut *conn)
        .await
        .map_err(internal_error)?;

    Ok((items, operations))
}

fn refuse_unsettled_work((items, operations): (i64, i64)) -> ServiceResult<()> {
    if items > 0 || operations > 0 {
        return Err(failed_precondition(format!(
            "funding policy cannot change while work that was accepted under the current \
             one is still in flight: {items} allocation item(s) and {operations} wallet \
             operation(s) are unfinished. Let them terminate, or cancel them, and retry"
        )));
    }
    Ok(())
}

pub(crate) async fn apply_setup_config(
    database: &Database,
    secret_store: &SecretStore,
    trust_fixtures_enabled: bool,
    local_iroh_node_id: Option<&str>,
    request: ApplySetupConfigRequest,
) -> ServiceResult<ApplySetupConfigResponse> {
    ensure_trust_fixtures_allow_network(trust_fixtures_enabled, request.config.network)?;
    let stored = load_setup_state(database).await?;
    ensure_gateway_identity_unchanged(stored.config.as_ref(), &request.config)?;
    ensure_funding_policy_settled(
        database,
        stored.config.as_ref(),
        &request.config.funding_policy,
    )
    .await?;
    // The gateway credential is not part of this write, and was never part of
    // what this config means — but the daemon cannot reach its gateway without
    // one, so requiring it here turns "nothing works and no screen says why"
    // into one sentence at the point of the mistake.
    if load_secret_record(database, GATEWAY_ADMIN_SECRET)
        .await?
        .is_none()
    {
        return Err(failed_precondition(
            "no gateway admin credential is stored: set it with set_config_secret before \
             applying setup config",
        ));
    }

    let has_bitcoind_password = load_secret_record(database, BITCOIND_PASSWORD_SECRET)
        .await?
        .is_some();
    let mut config = setup_config_to_view(&request.config, has_bitcoind_password)?;
    normalize_advertised_endpoint(database, &mut config, local_iroh_node_id).await?;

    let validation = validate_candidate_config(database, secret_store, &request.config).await?;
    let status = status_from_validation(&validation);

    let mut tx = database.begin_write().await.map_err(internal_error)?;
    ensure_funding_policy_settled_tx(
        &mut tx,
        stored.config.as_ref(),
        &request.config.funding_policy,
    )
    .await?;
    upsert_setup_state_tx(&mut tx, stored.revision, status, &config, &validation).await?;
    insert_audit_tx(&mut tx, "apply_setup_config").await?;
    tx.commit().await.map_err(internal_error)?;

    // Configuration is what readiness is derived from, so an operator
    // reconstructing "when did it stop advertising" starts here. The failing
    // check names are the answer to "why is it not ready", and they are short
    // and bounded; the config itself is not logged.
    let failed_checks = validation
        .checks
        .iter()
        .filter(|check| check.status != ValidationStatus::Passed)
        .map(|check| check.name.as_str())
        .collect::<Vec<_>>()
        .join(",");
    if status == SetupStatus::Ready {
        tracing::info!(%status, "applied setup config");
    } else {
        tracing::warn!(%status, failed_checks, "applied setup config; it is not ready");
    }

    Ok(ApplySetupConfigResponse { status, validation })
}

/// Asks a candidate gateway who it is.
///
/// The credential comes from the secret store, not the request: it is written
/// through [`set_config_secret`] first, so the probe carries no secret over the
/// wire and reaches the gateway with exactly the credential the daemon will use
/// afterwards.
pub(crate) async fn probe_gateway(
    database: &Database,
    secret_store: &SecretStore,
    request: ProbeGatewayRequest,
) -> ServiceResult<ProbeGatewayResponse> {
    if request.admin_url.trim().is_empty() {
        return Err(invalid_argument("gateway.admin_url is required"));
    }
    let credential = load_gateway_admin_credential(database, secret_store).await?;
    let probe = crate::gateway::probe_gateway_identity(&request.admin_url, credential)
        .await
        .map_err(|error| failed_precondition(format!("gateway did not answer: {error:#}")))?;

    Ok(ProbeGatewayResponse {
        gateway_id: probe.gateway_id,
        network: probe.network,
        lightning_alias: probe.lightning_alias,
    })
}

/// Writes one named secret, and nothing else.
///
/// Carrying secrets inside the whole-config write forces every write to answer
/// "what does absent mean?" for a value the operator may simply not have
/// retyped, and neither answer is right: treating an absent gateway credential
/// as a failure makes the dashboard demand it on every unrelated hard-field
/// edit, and treating an absent chain-observer password as **delete** loses an
/// operator's bitcoind password, and their chain connection with it, when they
/// change a gateway display name.
///
/// A named secret with an explicit operation has no absent case to interpret.
/// `Set` replaces, `Clear` removes, and a write that says neither does not
/// happen.
pub(crate) async fn set_config_secret(
    database: &Database,
    secret_store: &SecretStore,
    request: SetConfigSecretRequest,
) -> ServiceResult<SetConfigSecretResponse> {
    let name = secret_record_name(request.secret);

    let present = match &request.update {
        SecretUpdate::Set(SecretString(value)) => {
            if value.is_empty() {
                return Err(invalid_argument(format!(
                    "{} must not be empty: use the clear operation to remove it",
                    request.secret
                )));
            }
            let record = secret_store.encrypt(name, value).map_err(internal_error)?;
            let mut tx = database.begin_write().await.map_err(internal_error)?;
            upsert_secret_record_tx(&mut tx, name, &record).await?;
            insert_audit_tx(&mut tx, "set_config_secret").await?;
            tx.commit().await.map_err(internal_error)?;
            true
        }
        SecretUpdate::Clear => {
            // The gateway credential is the one secret the daemon cannot run
            // without: every wallet call authenticates with it, and clearing it
            // would stop payouts without changing any configuration that says
            // so. Removing a gateway is a config change, not a secret change.
            if request.secret == ConfigSecret::GatewayAdminCredential {
                return Err(invalid_argument(
                    "the gateway admin credential cannot be cleared: the daemon authenticates \
                     every gateway call with it. Replace it instead",
                ));
            }
            let mut tx = database.begin_write().await.map_err(internal_error)?;
            delete_secret_record_tx(&mut tx, name).await?;
            insert_audit_tx(&mut tx, "set_config_secret").await?;
            tx.commit().await.map_err(internal_error)?;
            false
        }
    };

    // The name and whether one is now stored, never the value and never its
    // length. Every wallet and chain-observer call authenticates with one of
    // these, so an operator reconstructing "when did it stop being able to
    // reach gatewayd" needs the write in the timeline.
    tracing::info!(
        secret = %request.secret,
        stored = present,
        "wrote a configuration secret"
    );

    // The stored view reports whether a chain-observer password exists, so the
    // write has to move it. Nothing else about the configuration changes.
    if request.secret == ConfigSecret::ChainObserverPassword {
        refresh_stored_bitcoind_password_flag(database, present).await?;
    }

    Ok(SetConfigSecretResponse {
        secret: request.secret,
        present,
    })
}

fn secret_record_name(secret: ConfigSecret) -> &'static str {
    match secret {
        ConfigSecret::GatewayAdminCredential => GATEWAY_ADMIN_SECRET,
        ConfigSecret::ChainObserverPassword => BITCOIND_PASSWORD_SECRET,
    }
}

/// How many times a derived write re-reads after losing the revision fence.
///
/// The two writers below derive their whole write from the row they just read —
/// a password flag, a node id — so a lost fence means their snapshot went
/// stale, not that anyone asked for something impossible. Re-reading gives the
/// right answer; refusing does not, because neither has an operator to refuse
/// to. `set_config_secret` has already committed the secret by the time the
/// flag is refreshed, and the Iroh adopter runs once per generation behind a
/// best-effort log at its call site. Abandoning either write leaves the stored
/// view disagreeing with the secret store, or the daemon advertising an address
/// it does not listen on, until the next restart.
///
/// The operator verbs are deliberately not retried. `apply_setup_config` and
/// `update_provider_config` carry a candidate the operator composed against a
/// specific stored config, so a lost fence means that composition is stale and
/// the refusal is the answer, not the failure.
///
/// The bound exists because these run inside a request and a background bind.
/// Each attempt is one read plus one `BEGIN IMMEDIATE`, and every attempt that
/// loses is a competing write that committed, so a sustained loss is a config
/// write storm rather than something a longer loop would settle.
const DERIVED_WRITE_ATTEMPTS: usize = 4;

/// Keeps the stored config view's `has_password` in step with the secret store.
///
/// The flag is a projection of the secret store, not operator input, so it is
/// derived here rather than waiting for the next config write to restate it.
/// A deployment with no config yet has nothing to update.
async fn refresh_stored_bitcoind_password_flag(
    database: &Database,
    has_password: bool,
) -> ServiceResult<()> {
    for _ in 0..DERIVED_WRITE_ATTEMPTS {
        let stored = load_setup_state(database).await?;
        let Some(mut config) = stored.config else {
            return Ok(());
        };
        let ChainObserverBackendView::Bitcoind {
            url,
            username,
            has_password: stored_flag,
        } = config.chain_observer.backend.clone()
        else {
            return Ok(());
        };
        if stored_flag == has_password {
            return Ok(());
        }
        config.chain_observer.backend = ChainObserverBackendView::Bitcoind {
            url,
            username,
            has_password,
        };

        let validation = stored
            .validation
            .clone()
            .unwrap_or_else(|| SetupValidationSummary {
                status: ValidationStatus::NotRun,
                checks: Vec::new(),
            });
        let mut tx = database.begin_write().await.map_err(internal_error)?;
        let written = try_upsert_setup_state_tx(
            &mut tx,
            stored.revision,
            stored.status,
            &config,
            &validation,
        )
        .await?;
        if written {
            tx.commit().await.map_err(internal_error)?;
            return Ok(());
        }
        // A config write landed between the read above and this transaction.
        // Its view of the backend is newer than ours, so re-derive against it
        // rather than overwrite it. The loop also re-tests the two early exits:
        // the newer view may already carry the flag we came to set.
        tx.rollback().await.map_err(internal_error)?;
    }
    Err(setup_revision_conflict())
}

pub(crate) async fn validate_setup(
    database: &Database,
    secret_store: &SecretStore,
    trust_fixtures_enabled: bool,
    request: ValidateSetupRequest,
) -> ServiceResult<ValidateSetupResponse> {
    let validation = match request.candidate_config {
        Some(config) => {
            ensure_trust_fixtures_allow_network(trust_fixtures_enabled, config.network)?;
            validate_candidate_config(database, secret_store, &config).await?
        }
        None => validate_current_setup(database, secret_store, ValidationReach::Network).await?,
    };

    Ok(ValidateSetupResponse { validation })
}

pub(crate) async fn update_provider_config(
    database: &Database,
    secret_store: &SecretStore,
    local_iroh_node_id: Option<&str>,
    request: UpdateProviderConfigRequest,
) -> ServiceResult<UpdateProviderConfigResponse> {
    let stored = load_setup_state(database).await?;
    let mut config = stored
        .config
        .ok_or_else(|| failed_precondition("setup config is not configured"))?;
    let previous = config.clone();
    apply_provider_patch(&mut config, request.patch);
    ensure_funding_policy_settled(database, Some(&previous), &config.funding_policy).await?;
    normalize_advertised_endpoint(database, &mut config, local_iroh_node_id).await?;
    let validation = validate_config_view(database, secret_store, &config).await?;
    let status = status_from_validation(&validation);

    let mut tx = database.begin_write().await.map_err(internal_error)?;
    ensure_funding_policy_settled_tx(&mut tx, Some(&previous), &config.funding_policy).await?;
    upsert_setup_state_tx(&mut tx, stored.revision, status, &config, &validation).await?;
    insert_audit_tx(&mut tx, "update_provider_config").await?;
    tx.commit().await.map_err(internal_error)?;

    Ok(UpdateProviderConfigResponse { config, validation })
}

/// Replaces an operator-submitted Iroh endpoint address with the daemon's own.
///
/// For an Iroh endpoint the address is not operator input at all: it is the
/// daemon's transport identity, derived from the provider signing key, which
/// the operator cannot know and cannot choose. Accepting whatever they sent and
/// then overwriting it at the next bind
/// ([`adopt_local_iroh_endpoint_address`]) made a write that looked like it
/// succeeded silently not stick, so writes are normalized on the way in
/// instead.
///
/// Before the transport binds there is no node id to substitute. The stored
/// address is kept in that case rather than cleared, because clearing it loses
/// a good value with no way back: [`adopt_local_iroh_endpoint_address`] runs
/// once per generation immediately after the bind, so a config write that
/// races the bind and lands *after* it would leave the daemon advertising
/// nothing until the next restart. Whatever is stored can only have been
/// written by a previous bind — operator input never reaches this field — and
/// the node id is stable across restarts by construction, so keeping it is
/// correct rather than merely safe. A deployment that has never bound has
/// nothing stored, so the address stays empty and readiness reports it missing.
///
/// Only the address, and only under Iroh. The other transports are placeholders
/// whose addresses (a URL, say) genuinely are operator-chosen, and every other
/// field of the endpoint — transport, discovery hints, protocol name — stays
/// operator-owned regardless.
async fn normalize_advertised_endpoint(
    database: &Database,
    config: &mut SetupConfigView,
    local_iroh_node_id: Option<&str>,
) -> ServiceResult<()> {
    if config.advertised_endpoint.transport != RpcTransport::Iroh {
        return Ok(());
    }

    let address = match local_iroh_node_id {
        Some(node_id) => node_id.to_owned(),
        None => load_setup_state(database)
            .await?
            .config
            .map(|stored| stored.advertised_endpoint.address.0)
            .unwrap_or_default(),
    };
    config.advertised_endpoint.address = RpcEndpointAddress(address);
    Ok(())
}

/// Records the local public Iroh node id as the advertised endpoint address.
///
/// The address is not operator-chosen data: it *is* the daemon's transport
/// identity, which the operator cannot know before the daemon derives it. The
/// readiness gate requires the persisted address to equal the local node id, so
/// without this the first start after configuring the endpoint — and every
/// start before the key became derived rather than random — left the deployment
/// unready until an operator re-applied setup config by hand.
///
/// Returns whether the stored address changed. Only ever touches the address of
/// an Iroh-transport endpoint; anything else is left to the operator.
pub(crate) async fn adopt_local_iroh_endpoint_address(
    database: &Database,
    node_id: &str,
) -> ServiceResult<bool> {
    for _ in 0..DERIVED_WRITE_ATTEMPTS {
        let stored = load_setup_state(database).await?;
        let Some(mut config) = stored.config else {
            return Ok(false);
        };
        if config.advertised_endpoint.transport != RpcTransport::Iroh
            || config.advertised_endpoint.address.0 == node_id
        {
            return Ok(false);
        }

        let previous = config.advertised_endpoint.address.0.clone();
        config.advertised_endpoint.address = RpcEndpointAddress(node_id.to_owned());
        let validation = stored
            .validation
            .clone()
            .unwrap_or_else(|| summary(Vec::new()));

        let mut tx = database.begin_write().await.map_err(internal_error)?;
        let written = try_upsert_setup_state_tx(
            &mut tx,
            stored.revision,
            stored.status,
            &config,
            &validation,
        )
        .await?;
        if !written {
            // A config write landed between the read above and this
            // transaction. Overwriting it here would revert it, and the write
            // this adopter races is `normalize_advertised_endpoint`, which
            // substitutes this same node id when the transport is already
            // bound — so the re-read may well find nothing left to do.
            tx.rollback().await.map_err(internal_error)?;
            continue;
        }
        insert_audit_tx(&mut tx, "adopt_local_iroh_endpoint_address").await?;
        tx.commit().await.map_err(internal_error)?;

        tracing::info!(
            previous_address = %previous,
            node_id,
            "adopted the local public Iroh node id as the advertised endpoint address"
        );
        return Ok(true);
    }
    Err(setup_revision_conflict())
}

pub(crate) async fn load_setup_state(database: &Database) -> ServiceResult<StoredSetupState> {
    let row = sqlx::query(
        "SELECT status, config_view_json, latest_validation_json, revision \
         FROM setup_state WHERE id = ?",
    )
    .bind(SETUP_STATE_ID)
    .fetch_optional(database.pool())
    .await
    .map_err(internal_error)?;

    let Some(row) = row else {
        return Ok(StoredSetupState {
            status: SetupStatus::NotConfigured,
            config: None,
            validation: None,
            revision: 0,
        });
    };
    let revision: i64 = row.get("revision");

    let status = parse_setup_status(row.get::<String, _>("status").as_str())?;
    let config = match row.get::<Option<String>, _>("config_view_json") {
        Some(json) => {
            let mut config: SetupConfigView =
                serde_json::from_str(&json).map_err(internal_error)?;
            config.attestation_summary = crate::attestation_store::summary(database).await?;
            Some(config)
        }
        None => None,
    };
    let validation = match row.get::<Option<String>, _>("latest_validation_json") {
        Some(json) => Some(serde_json::from_str(&json).map_err(internal_error)?),
        None => None,
    };

    Ok(StoredSetupState {
        status,
        config,
        validation,
        revision,
    })
}

/// The setup row's current revision, read inside the caller's transaction.
///
/// Pairs with [`StoredSetupState::revision`] so a caller holding a snapshot
/// across an await can fence its own commit against an Admin update that
/// superseded that snapshot. Reading it in the transaction is the whole point:
/// a read outside would be one more racy check rather than a fence.
pub(crate) async fn setup_revision_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
) -> ServiceResult<i64> {
    let revision: Option<i64> = sqlx::query_scalar("SELECT revision FROM setup_state WHERE id = ?")
        .bind(SETUP_STATE_ID)
        .fetch_optional(&mut **tx)
        .await
        .map_err(internal_error)?;
    Ok(revision.unwrap_or(0))
}

/// Fails unless every stored secret decrypts with `secret_store`.
///
/// Restore uses this as a precondition rather than a validation check. The
/// other checks ask whether the world is ready — a disaster-recovery host has
/// its gatewayd down and must still be restorable — but this one asks whether
/// this daemon can read the payload at all. A data dir whose records were
/// written under a different key comes up looking healthy and then fails every
/// operation that needs a secret, including the admin token load that guards
/// the whole Admin API, which locks the operator out of the daemon they just
/// restored.
///
/// Every record is checked, not just the configured dependencies: a backup with
/// no gateway configured but a rotated admin token still has to be readable.
pub(crate) async fn ensure_secret_records_decryptable(
    database: &Database,
    secret_store: &SecretStore,
) -> ServiceResult<()> {
    let rows = sqlx::query(
        "SELECT name, version, algorithm, key_id, nonce, ciphertext \
         FROM secret_records ORDER BY name",
    )
    .fetch_all(database.pool())
    .await
    .map_err(internal_error)?;

    let mut unreadable = Vec::new();
    for row in rows {
        let name: String = row.get("name");
        let record = EncryptedSecretRecord {
            version: row.get("version"),
            algorithm: row.get("algorithm"),
            key_id: row.get("key_id"),
            nonce: row.get("nonce"),
            ciphertext: row.get("ciphertext"),
        };
        if let Err(error) = secret_store.decrypt(&name, &record) {
            unreadable.push(format!("{name} ({error})"));
        }
    }

    if unreadable.is_empty() {
        return Ok(());
    }
    Err(failed_precondition(format!(
        "secret records cannot be decrypted with this daemon's secret-store key: {}. \
         The archive was written under a different key; restore it with that key \
         (--secret-store-key / FLIP_SECRET_STORE_KEY) or from a host that has it.",
        unreadable.join(", ")
    )))
}

async fn load_secret_record(
    database: &Database,
    secret_name: &str,
) -> ServiceResult<Option<EncryptedSecretRecord>> {
    let row = sqlx::query(
        "SELECT version, algorithm, key_id, nonce, ciphertext \
         FROM secret_records WHERE name = ?",
    )
    .bind(secret_name)
    .fetch_optional(database.pool())
    .await
    .map_err(internal_error)?;

    Ok(row.map(|row| EncryptedSecretRecord {
        version: row.get("version"),
        algorithm: row.get("algorithm"),
        key_id: row.get("key_id"),
        nonce: row.get("nonce"),
        ciphertext: row.get("ciphertext"),
    }))
}

/// Loads the setup config and gateway admin credential once setup is Ready.
pub(crate) async fn ready_gateway_config(
    database: &Database,
    secret_store: &SecretStore,
) -> ServiceResult<(SetupConfigView, String)> {
    let StoredSetupState { status, config, .. } = load_setup_state(database).await?;
    let config = config.ok_or_else(|| failed_precondition("setup config is not configured"))?;
    if status != SetupStatus::Ready {
        return Err(failed_precondition("setup is not ready"));
    }
    let credential = load_gateway_admin_credential(database, secret_store).await?;
    Ok((config, credential))
}

pub(crate) async fn load_gateway_admin_credential(
    database: &Database,
    secret_store: &SecretStore,
) -> ServiceResult<String> {
    load_required_secret(database, secret_store, GATEWAY_ADMIN_SECRET).await
}

pub(crate) async fn load_bitcoind_password(
    database: &Database,
    secret_store: &SecretStore,
) -> ServiceResult<Option<String>> {
    match load_secret_record(database, BITCOIND_PASSWORD_SECRET).await? {
        Some(record) => Ok(Some(
            secret_store
                .decrypt(BITCOIND_PASSWORD_SECRET, &record)
                .map_err(internal_error)?,
        )),
        None => Ok(None),
    }
}

async fn load_required_secret(
    database: &Database,
    secret_store: &SecretStore,
    secret_name: &str,
) -> ServiceResult<String> {
    let record = load_secret_record(database, secret_name)
        .await?
        .ok_or_else(|| failed_precondition(format!("secret record {secret_name} is missing")))?;
    let value = secret_store
        .decrypt(secret_name, &record)
        .map_err(internal_error)?;
    if value.is_empty() {
        return Err(failed_precondition(format!(
            "secret record {secret_name} decrypts to an empty value"
        )));
    }
    Ok(value)
}

async fn upsert_secret_record_tx(
    tx: &mut Transaction<'_, Sqlite>,
    secret_name: &str,
    record: &EncryptedSecretRecord,
) -> ServiceResult<()> {
    secret_store::upsert_secret_record(secret_name, record)
        .execute(&mut **tx)
        .await
        .map_err(internal_error)?;

    Ok(())
}

async fn delete_secret_record_tx(
    tx: &mut Transaction<'_, Sqlite>,
    secret_name: &str,
) -> ServiceResult<()> {
    sqlx::query("DELETE FROM secret_records WHERE name = ?")
        .bind(secret_name)
        .execute(&mut **tx)
        .await
        .map_err(internal_error)?;
    Ok(())
}

/// Persists the setup row, but only on top of the revision the caller read.
///
/// Every caller reads the row, derives a whole `SetupConfigView` from it, and
/// only then opens the transaction that writes it back. For the two operator
/// verbs that interval spans `normalize_advertised_endpoint` and
/// `validate_candidate_config` — network calls, so seconds. Without a fence the
/// write is a blind overwrite of whatever is there now, so a config change that
/// commits inside that interval is silently reverted, and an item admitted
/// under the new policy settles under the old one.
///
/// That is also what the funding-policy guard could not see on its own.
/// [`ensure_funding_policy_settled_tx`] decides whether to count anything by
/// asking [`funding_policy_changes`], which compares the candidate against the
/// caller's *pre-image*. When that pre-image is stale the predicate is answered
/// about a policy that is no longer stored, and the guard returns `Ok` without
/// counting. This fence removes the case rather than teaching the guard about
/// it: a write built on a stale read does not happen at all, so the guard is
/// never asked to rule on one.
///
/// It also makes the two paths that carry no funding-policy guard sound.
/// [`refresh_stored_bitcoind_password_flag`] and
/// [`adopt_local_iroh_endpoint_address`] each move one field and write the
/// whole view back, so without the fence they would revert a policy change they
/// never inspected. They do not inspect it, and they cannot revert it.
///
/// The fence lives on the writing statement rather than beside it, so no caller
/// can take this path without it and no later edit can separate the two.
/// `SPEC-flip-admin-api.md:107` promises hot settings are "persisted
/// atomically"; this is that promise.
async fn upsert_setup_state_tx(
    tx: &mut Transaction<'_, Sqlite>,
    expected_revision: i64,
    status: SetupStatus,
    config: &SetupConfigView,
    validation: &SetupValidationSummary,
) -> ServiceResult<()> {
    if try_upsert_setup_state_tx(tx, expected_revision, status, config, validation).await? {
        return Ok(());
    }
    Err(setup_revision_conflict())
}

/// The fenced write, reporting a lost fence rather than raising it.
///
/// Callers that derive their write entirely from the row they just read use
/// this and read again; see [`DERIVED_WRITE_ATTEMPTS`].
async fn try_upsert_setup_state_tx(
    tx: &mut Transaction<'_, Sqlite>,
    expected_revision: i64,
    status: SetupStatus,
    config: &SetupConfigView,
    validation: &SetupValidationSummary,
) -> ServiceResult<bool> {
    let config_json = serde_json::to_string(config).map_err(internal_error)?;
    let validation_json = serde_json::to_string(validation).map_err(internal_error)?;

    // The predicate on `DO UPDATE` fences every write onto an existing row, and
    // `RETURNING` fences the insert arm, which no `WHERE` here can reach: an
    // upsert's conflict target only decides between inserting and updating, so
    // gating the candidate row would remove the update arm along with it.
    //
    // The returned revision decides all four cases against one test. A write
    // that lands leaves `expected_revision + 1`, whether it inserted over the
    // absent row that `load_setup_state` reports as revision 0, or updated the
    // revision it read. A write refused by the predicate returns no row at all.
    // And an insert against a non-zero expectation — a caller holding a
    // revision the row does not have, which today needs a `DELETE FROM
    // setup_state` that does not exist — returns 1, which is not what it
    // expected, so it is refused rather than silently starting over.
    let written: Option<i64> = sqlx::query_scalar(
        "INSERT INTO setup_state \
         (id, status, config_view_json, latest_validation_json, revision, created_at, updated_at) \
         VALUES (?, ?, ?, ?, 1, unixepoch(), unixepoch()) \
         ON CONFLICT(id) DO UPDATE SET \
           status = excluded.status, \
           config_view_json = excluded.config_view_json, \
           latest_validation_json = excluded.latest_validation_json, \
           revision = setup_state.revision + 1, \
           updated_at = unixepoch() \
         WHERE setup_state.revision = ? \
         RETURNING revision",
    )
    .bind(SETUP_STATE_ID)
    .bind(status.to_string())
    .bind(config_json)
    .bind(validation_json)
    .bind(expected_revision)
    .fetch_optional(&mut **tx)
    .await
    .map_err(internal_error)?;

    Ok(written == Some(expected_revision + 1))
}

fn setup_revision_conflict() -> ServiceError {
    failed_precondition(
        "the stored setup configuration changed while this update was being prepared, so \
         applying it would revert that change. Re-read the current configuration and apply \
         the update again",
    )
}

async fn insert_audit_tx(tx: &mut Transaction<'_, Sqlite>, action: &str) -> ServiceResult<()> {
    sqlx::query(
        "INSERT INTO audit_log (action, detail_json, created_at) VALUES (?, NULL, unixepoch())",
    )
    .bind(action)
    .execute(&mut **tx)
    .await
    .map_err(internal_error)?;
    Ok(())
}

/// Projects an operator-supplied config into the stored, secret-free shape.
///
/// `has_bitcoind_password` is supplied by the caller rather than read off the
/// config, because the password is not in the config: it is stored under its own
/// name and written through [`set_config_secret`]. The view reports whether one
/// exists, which is all a client is ever told about it.
fn setup_config_to_view(
    config: &SetupConfig,
    has_bitcoind_password: bool,
) -> ServiceResult<SetupConfigView> {
    let gateway_id = config
        .gateway
        .gateway_id
        .clone()
        .ok_or_else(|| invalid_argument("gateway.gateway_id is required in Phase 1"))?;
    Ok(SetupConfigView {
        network: config.network,
        gateway: GatewayConfigView {
            gateway_id,
            gateway_name: config.gateway.gateway_name.clone(),
            admin_url: config.gateway.admin_url.clone(),
            has_admin_credential: true,
            identity_metadata: config.gateway.identity_metadata.clone(),
        },
        chain_observer: chain_observer_view(&config.chain_observer, has_bitcoind_password),
        relays: config.relays.clone(),
        capacity: config.capacity.clone(),
        funding_policy: config.funding_policy.clone(),
        replenishment: config.replenishment.clone(),
        advertised_endpoint: config.advertised_endpoint.clone(),
        advertisement: config.advertisement.clone(),
        provider_display: config.provider_display.clone(),
        policy: config.policy.clone(),
        attestation_summary: AttestationSummary::default(),
    })
}

fn chain_observer_view(
    config: &ChainObserverConfig,
    has_password: bool,
) -> ChainObserverConfigView {
    let backend = match &config.backend {
        ChainObserverBackend::Esplora { url } => {
            ChainObserverBackendView::Esplora { url: url.clone() }
        }
        ChainObserverBackend::Bitcoind { url, username } => ChainObserverBackendView::Bitcoind {
            url: url.clone(),
            username: username.clone(),
            has_password,
        },
    };

    ChainObserverConfigView { backend }
}

fn apply_provider_patch(config: &mut SetupConfigView, patch: ProviderConfigPatch) {
    if let Some(policy) = patch.policy {
        config.policy = policy;
    }
    if let Some(relays) = patch.relays {
        config.relays = relays;
    }
    if let Some(capacity) = patch.capacity {
        config.capacity = capacity;
    }
    if let Some(funding_policy) = patch.funding_policy {
        config.funding_policy = funding_policy;
    }
    if let Some(replenishment) = patch.replenishment {
        config.replenishment = replenishment;
    }
    if let Some(advertised_endpoint) = patch.advertised_endpoint {
        config.advertised_endpoint = advertised_endpoint;
    }
    if let Some(advertisement) = patch.advertisement {
        config.advertisement = advertisement;
    }
    if let Some(provider_display) = patch.provider_display {
        match provider_display {
            ProviderDisplayPatch::Set(display) => config.provider_display = Some(display),
            ProviderDisplayPatch::Clear => config.provider_display = None,
        }
    }
}

/// Validates a configuration the operator is proposing.
///
/// The secrets it needs are read from the store, not from the candidate. They
/// are not part of a configuration and are written through
/// [`set_config_secret`]: a config write states the whole configuration, so a
/// secret carried inside one has an absent case, and interpreting that absence
/// is what silently deleted an operator's bitcoind password.
///
/// A consequence worth naming: this validates the candidate against the
/// *stored* credentials. Testing a different gateway's credential means storing
/// it first, which is the same order the wizard follows anyway.
async fn validate_candidate_config(
    database: &Database,
    secret_store: &SecretStore,
    config: &SetupConfig,
) -> ServiceResult<SetupValidationSummary> {
    let mut checks = Vec::new();
    checks.push(secret_store_probe(secret_store));
    checks.push(gateway_candidate_check(config));
    checks.push(chain_observer_candidate_check(&config.chain_observer));
    checks.push(provider_display_check(config.provider_display.as_ref()));
    checks.push(provider_policy_check(config.network, &config.policy));
    checks.push(funding_policy_check(&config.funding_policy));
    checks.push(advertisement_config_check(&config.advertisement));
    checks.push(reachability_check("gateway_reachable", &config.gateway.admin_url).await);
    checks.push(chain_observer_reachability_candidate(&config.chain_observer).await);
    checks.push(gateway_api_candidate_check(database, secret_store, config).await);
    checks.push(chain_observer_api_candidate_check(database, secret_store, config).await);
    Ok(summary(checks))
}

async fn validate_current_setup(
    database: &Database,
    secret_store: &SecretStore,
    reach: ValidationReach,
) -> ServiceResult<SetupValidationSummary> {
    let stored = load_setup_state(database).await?;
    let Some(config) = stored.config else {
        return Ok(summary(vec![check(
            "setup_config",
            ValidationStatus::Failed,
            Some("setup config is not configured".to_owned()),
        )]));
    };

    validate_config_view_with_reach(database, secret_store, &config, reach).await
}

/// Validates the state a restore has staged, without opening a socket.
///
/// Restore calls this instead of `validate_setup` so that a restore-mode process
/// makes no outbound connection derived from the archive it was handed.
pub(crate) async fn validate_restored_setup(
    database: &Database,
    secret_store: &SecretStore,
) -> ServiceResult<SetupValidationSummary> {
    validate_current_setup(database, secret_store, ValidationReach::Local).await
}

/// Whether a validation run may talk to the hosts the configuration names.
///
/// Restore mode passes `Local`. A restore-mode process validates an archive it
/// has been handed, and the hosts in that archive are not this daemon's current
/// configuration — dialling them would make a recovery depend on a gateway that
/// is frequently the very thing that is down, and would send the archive's own
/// gateway admin credential to the archive's own URL with no endpoint policy
/// applied.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ValidationReach {
    /// Local checks only: no socket is opened.
    Local,
    /// Local checks plus reachability and backend API probes.
    Network,
}

async fn validate_config_view(
    database: &Database,
    secret_store: &SecretStore,
    config: &SetupConfigView,
) -> ServiceResult<SetupValidationSummary> {
    validate_config_view_with_reach(database, secret_store, config, ValidationReach::Network).await
}

async fn validate_config_view_with_reach(
    database: &Database,
    secret_store: &SecretStore,
    config: &SetupConfigView,
    reach: ValidationReach,
) -> ServiceResult<SetupValidationSummary> {
    let mut checks = Vec::new();
    checks.push(decryptability_check(database, secret_store, GATEWAY_ADMIN_SECRET).await);
    if chain_observer_has_password(config) {
        checks.push(decryptability_check(database, secret_store, BITCOIND_PASSWORD_SECRET).await);
    }
    checks.push(provider_display_check(config.provider_display.as_ref()));
    checks.push(provider_policy_check(config.network, &config.policy));
    checks.push(funding_policy_check(&config.funding_policy));
    checks.push(advertisement_config_check(&config.advertisement));
    // The four network checks. Every one of them opens a socket to a host named
    // by `config`, and `gateway_api_view_check` additionally sends the stored
    // gateway admin credential and calls `allocate_deposit_address`, which
    // changes state on that gateway's Lightning node.
    if reach == ValidationReach::Network {
        checks.push(reachability_check("gateway_reachable", &config.gateway.admin_url).await);
        checks.push(chain_observer_reachability_view(&config.chain_observer).await);
        checks.push(gateway_api_view_check(database, secret_store, config).await);
        checks.push(
            chain_observer_api_view_check(database, secret_store, &config.chain_observer).await,
        );
    }
    checks.push(active_liability_check(database, config).await?);
    Ok(summary(checks))
}

/// Reports the configuration being applied against the liabilities already
/// outstanding under it.
///
/// A configuration update can lower the allocation cap or raise the fee reserve
/// below or above what active allocations and sends already commit, and until
/// this check it did so without comparing the two at all — the operator saw a
/// clean validation summary for a configuration their own deployment already
/// exceeded.
///
/// It reports rather than refuses, and that is deliberate. Lowering the cap
/// below outstanding reservations is how an operator winds a provider down, and
/// refusing it would remove the only way to stop taking new work without
/// stopping the daemon. Nor is it unsafe: `plan_allocation` subtracts active
/// reservations from the cap with a saturating subtraction and takes the
/// minimum with the wallet-backed figure, so a cap beneath outstanding
/// reservations admits nothing rather than admitting too much. What was missing
/// was the operator being told.
async fn active_liability_check(
    database: &Database,
    config: &SetupConfigView,
) -> ServiceResult<SetupValidationCheck> {
    let mut tx = database.begin_write().await.map_err(internal_error)?;
    let reserved = crate::wallet::active_reserved_amount_tx(&mut tx).await?;
    let outgoing = crate::wallet::active_wallet_withdrawal_amount_tx(&mut tx).await?;
    tx.commit().await.map_err(internal_error)?;
    let observed = crate::wallet::latest_wallet_balance_observation(database)
        .await?
        .map(|balance| balance.spendable);

    let mut exceeded = Vec::new();
    if let Some(cap) = config.capacity.explicit_cap
        && reserved.0 > cap.0
    {
        exceeded.push(format!(
            "active allocation reservations {} exceed the configured cap {}",
            reserved.0, cap.0
        ));
    }
    let committed = reserved
        .0
        .saturating_add(outgoing.0)
        .saturating_add(config.funding_policy.fee_reserve.0);
    if let Some(observed) = observed
        && committed > observed.0
    {
        exceeded.push(format!(
            "reservations, outstanding sends, and the fee reserve total {committed} against an \
             observed spendable balance of {}",
            observed.0
        ));
    }

    let detail = if exceeded.is_empty() {
        "outstanding reservations and sends fit inside this configuration".to_owned()
    } else {
        format!(
            "this configuration is already exceeded by outstanding work, so no new allocation \
             will be admitted until it drains: {}",
            exceeded.join("; ")
        )
    };
    Ok(check(
        "active_liabilities",
        ValidationStatus::Passed,
        Some(detail),
    ))
}

fn secret_store_probe(secret_store: &SecretStore) -> SetupValidationCheck {
    match secret_store
        .encrypt("validation.probe", "probe")
        .and_then(|record| secret_store.decrypt("validation.probe", &record))
    {
        Ok(value) if value == "probe" => check(
            "secret_store",
            ValidationStatus::Passed,
            Some("secret-store key can encrypt and decrypt".to_owned()),
        ),
        Ok(_) => check(
            "secret_store",
            ValidationStatus::Failed,
            Some("secret-store probe returned unexpected plaintext".to_owned()),
        ),
        Err(error) => check(
            "secret_store",
            ValidationStatus::Failed,
            Some(error.to_string()),
        ),
    }
}

fn provider_display_check(
    display: Option<&fedi_decentralized_service_liquidity_manager::ProviderDisplay>,
) -> SetupValidationCheck {
    match display.map(|display| display.validate()) {
        None => check(
            "provider_display",
            ValidationStatus::Passed,
            Some("no provider display metadata is configured".to_owned()),
        ),
        Some(Ok(())) => check(
            "provider_display",
            ValidationStatus::Passed,
            Some("provider display metadata is within the advertisement limits".to_owned()),
        ),
        Some(Err(error)) => check(
            "provider_display",
            ValidationStatus::Failed,
            Some(format!("provider display metadata is invalid: {error}")),
        ),
    }
}

/// Checks the accepted attester list, and that the policy serves the network
/// the provider is configured for.
///
/// `policy.supported_networks` gates every public request: `public_service`
/// refuses a request whose network is absent from it, and FIs discard the
/// advertisement earlier still, on the same list. Nothing kept it in agreement
/// with `config.network`. The list has no editor of its own — the dashboard
/// wrote it as a side effect of editing the accepted attesters — so changing
/// the network alone left the two disagreeing.
///
/// The result was a deployment that validated clean, published, and reported
/// itself ready, then refused every request it received while receiving almost
/// none, with nothing on the operator's screen to say why.
fn provider_policy_check(network: BitcoinNetwork, policy: &ProviderPolicy) -> SetupValidationCheck {
    if policy.accepted_attester_policies.is_empty() {
        return check(
            "provider_policy",
            ValidationStatus::Failed,
            Some("at least one accepted attester policy is required".to_owned()),
        );
    }

    if !policy.supported_networks.contains(&network) {
        return check(
            "provider_policy",
            ValidationStatus::Failed,
            Some(format!(
                "policy.supported_networks must contain the configured network {network}: \
                 the provider would refuse every request it is configured to serve"
            )),
        );
    }

    check(
        "provider_policy",
        ValidationStatus::Passed,
        Some(
            "provider policy has at least one accepted attester and serves the configured network"
                .to_owned(),
        ),
    )
}

fn funding_policy_check(config: &FundingPolicyConfig) -> SetupValidationCheck {
    if config.stability_pool_min_fee_rate_ppb > MAX_STABILITY_POOL_FEE_RATE_PPB {
        return check(
            "funding_policy",
            ValidationStatus::Failed,
            Some(format!(
                "funding_policy.stability_pool_min_fee_rate_ppb must be <= {MAX_STABILITY_POOL_FEE_RATE_PPB}"
            )),
        );
    }

    check(
        "funding_policy",
        ValidationStatus::Passed,
        Some("funding policy has valid stability-pool fee settings".to_owned()),
    )
}

/// Refuses a republish interval of zero.
///
/// A published advertisement expires at `issued_at + republish_interval * 2`,
/// so at zero it expires in the instant it is issued and every FI discards it
/// on `expires_at <= now` before reading anything else. The failure is silent
/// in both directions: the daemon records the publication as successful and
/// the operator's dashboard stays green, while no customer ever sees the
/// provider.
///
/// Both config write paths need it. The wizard rejects zero in its own
/// validator, but Settings mounts the same field without one and parses a
/// free-text box, so an emptied box arrives here as zero — through
/// `update_provider_config`, which the wizard's validator never sees.
///
/// Reported as a failed check rather than refused outright, like every other
/// check here: a failed check leaves [`SetupStatus::PendingValidation`], which
/// `public_readiness` refuses to publish under. The operator keeps the rest of
/// their edit and sees the reason instead of losing the write.
fn advertisement_config_check(config: &AdvertisementConfig) -> SetupValidationCheck {
    if config.republish_interval.0 == 0 {
        return check(
            "advertisement_config",
            ValidationStatus::Failed,
            Some(
                "advertisement.republish_interval must be greater than zero: an \
                 advertisement published at zero expires when it is issued"
                    .to_owned(),
            ),
        );
    }

    check(
        "advertisement_config",
        ValidationStatus::Passed,
        Some("advertisement republishes on a non-zero interval".to_owned()),
    )
}

fn gateway_candidate_check(config: &SetupConfig) -> SetupValidationCheck {
    let detail = if config.gateway.gateway_id.is_none() {
        Some("gateway.gateway_id is required".to_owned())
    } else if config.gateway.admin_url.is_empty() {
        Some("gateway.admin_url is required".to_owned())
    } else {
        None
    };

    match detail {
        Some(detail) => check("gateway_config", ValidationStatus::Failed, Some(detail)),
        None => check(
            "gateway_config",
            ValidationStatus::Passed,
            Some("gateway config has required fields".to_owned()),
        ),
    }
}

fn chain_observer_candidate_check(config: &ChainObserverConfig) -> SetupValidationCheck {
    let detail = match &config.backend {
        ChainObserverBackend::Esplora { url } if url.0.is_empty() => {
            Some("chain_observer.esplora.url is required".to_owned())
        }
        ChainObserverBackend::Bitcoind { url, .. } if url.0.is_empty() => {
            Some("chain_observer.bitcoind.url is required".to_owned())
        }
        _ => None,
    };

    match detail {
        Some(detail) => check(
            "chain_observer_config",
            ValidationStatus::Failed,
            Some(detail),
        ),
        None => check(
            "chain_observer_config",
            ValidationStatus::Passed,
            Some("chain observer config has required fields".to_owned()),
        ),
    }
}

async fn decryptability_check(
    database: &Database,
    secret_store: &SecretStore,
    secret_name: &str,
) -> SetupValidationCheck {
    let record = match load_secret_record(database, secret_name).await {
        Ok(Some(record)) => record,
        Ok(None) => {
            return check(
                secret_name,
                ValidationStatus::Failed,
                Some("secret record is missing".to_owned()),
            );
        }
        Err(error) => {
            return check(
                secret_name,
                ValidationStatus::Failed,
                Some(error.to_string()),
            );
        }
    };

    match secret_store.decrypt(secret_name, &record) {
        Ok(value) if !value.is_empty() => check(
            secret_name,
            ValidationStatus::Passed,
            Some("secret record decrypts".to_owned()),
        ),
        Ok(_) => check(
            secret_name,
            ValidationStatus::Failed,
            Some("secret record decrypts to an empty value".to_owned()),
        ),
        Err(error) => check(
            secret_name,
            ValidationStatus::Failed,
            Some(error.to_string()),
        ),
    }
}

async fn chain_observer_reachability_candidate(
    config: &ChainObserverConfig,
) -> SetupValidationCheck {
    match &config.backend {
        ChainObserverBackend::Esplora { url } => {
            reachability_check("chain_observer_reachable", &url.0).await
        }
        ChainObserverBackend::Bitcoind { url, .. } => {
            reachability_check("chain_observer_reachable", &url.0).await
        }
    }
}

async fn chain_observer_reachability_view(
    config: &ChainObserverConfigView,
) -> SetupValidationCheck {
    match &config.backend {
        ChainObserverBackendView::Esplora { url } => {
            reachability_check("chain_observer_reachable", &url.0).await
        }
        ChainObserverBackendView::Bitcoind { url, .. } => {
            reachability_check("chain_observer_reachable", &url.0).await
        }
    }
}

async fn gateway_api_candidate_check(
    database: &Database,
    secret_store: &SecretStore,
    config: &SetupConfig,
) -> SetupValidationCheck {
    let credential = match load_gateway_admin_credential(database, secret_store).await {
        Ok(credential) => credential,
        Err(error) => {
            return check(
                "gateway_wallet_api",
                ValidationStatus::Failed,
                Some(error.to_string()),
            );
        }
    };
    let view = match setup_config_to_view(config, false) {
        Ok(view) => view,
        Err(error) => {
            return check(
                "gateway_wallet_api",
                ValidationStatus::Failed,
                Some(error.to_string()),
            );
        }
    };
    gateway_api_check(view, credential).await
}

async fn gateway_api_view_check(
    database: &Database,
    secret_store: &SecretStore,
    config: &SetupConfigView,
) -> SetupValidationCheck {
    match load_gateway_admin_credential(database, secret_store).await {
        Ok(credential) => gateway_api_check(config.clone(), credential).await,
        Err(error) => check(
            "gateway_wallet_api",
            ValidationStatus::Failed,
            Some(error.to_string()),
        ),
    }
}

async fn gateway_api_check(config: SetupConfigView, credential: String) -> SetupValidationCheck {
    let result = timeout(GATEWAY_API_VALIDATION_TIMEOUT, async {
        let wallet = GatewaydFundsWallet::new(config.clone(), credential).await?;
        let network = wallet.network().await?;
        if network != config.network {
            anyhow::bail!(
                "gatewayd network {network} does not match configured network {}",
                config.network
            );
        }
        wallet.balance_summary().await?;
        wallet
            .allocate_deposit_address(&WalletOperationId("validation".to_owned()), None)
            .await?;
        anyhow::Ok(())
    })
    .await;

    match result {
        Ok(Ok(())) => check(
            "gateway_wallet_api",
            ValidationStatus::Passed,
            Some("gatewayd wallet API is reachable and network-compatible".to_owned()),
        ),
        Ok(Err(error)) => check(
            "gateway_wallet_api",
            ValidationStatus::Failed,
            Some(error.to_string()),
        ),
        Err(_) => check(
            "gateway_wallet_api",
            ValidationStatus::Failed,
            Some("timed out validating gatewayd wallet API".to_owned()),
        ),
    }
}

async fn chain_observer_api_candidate_check(
    database: &Database,
    secret_store: &SecretStore,
    config: &SetupConfig,
) -> SetupValidationCheck {
    let password = match load_bitcoind_password(database, secret_store).await {
        Ok(password) => password,
        Err(error) => {
            return check(
                "chain_observer_api",
                ValidationStatus::Failed,
                Some(error.to_string()),
            );
        }
    };
    let view = chain_observer_view(&config.chain_observer, password.is_some());
    chain_observer_api_check(&view, password).await
}

async fn chain_observer_api_view_check(
    database: &Database,
    secret_store: &SecretStore,
    config: &ChainObserverConfigView,
) -> SetupValidationCheck {
    let password = match load_bitcoind_password(database, secret_store).await {
        Ok(password) => password,
        Err(error) => {
            return check(
                "chain_observer_api",
                ValidationStatus::Failed,
                Some(error.to_string()),
            );
        }
    };
    chain_observer_api_check(config, password).await
}

async fn chain_observer_api_check(
    config: &ChainObserverConfigView,
    password: Option<String>,
) -> SetupValidationCheck {
    let observer = ConfiguredChainObserver::from_config(config, password);
    match timeout(CHAIN_OBSERVER_API_VALIDATION_TIMEOUT, observer.health()).await {
        Ok(Ok(health)) if health.reachable => check(
            "chain_observer_api",
            ValidationStatus::Passed,
            health.detail,
        ),
        Ok(Ok(health)) => check(
            "chain_observer_api",
            ValidationStatus::Failed,
            health.detail,
        ),
        Ok(Err(error)) => check(
            "chain_observer_api",
            ValidationStatus::Failed,
            Some(error.to_string()),
        ),
        Err(_) => check(
            "chain_observer_api",
            ValidationStatus::Failed,
            Some("timed out validating chain observer API".to_owned()),
        ),
    }
}

async fn reachability_check(name: &str, raw_url: &str) -> SetupValidationCheck {
    let endpoint = match parse_tcp_endpoint(raw_url) {
        Ok(endpoint) => endpoint,
        Err(error) => return check(name, ValidationStatus::Failed, Some(error.to_string())),
    };

    match timeout(
        TCP_REACHABILITY_TIMEOUT,
        TcpStream::connect((endpoint.host.as_str(), endpoint.port)),
    )
    .await
    {
        Ok(Ok(_)) => check(
            name,
            ValidationStatus::Passed,
            Some(format!("reachable at {}:{}", endpoint.host, endpoint.port)),
        ),
        Ok(Err(error)) => check(name, ValidationStatus::Failed, Some(error.to_string())),
        Err(_) => check(
            name,
            ValidationStatus::Failed,
            Some(format!(
                "timed out connecting to {}:{}",
                endpoint.host, endpoint.port
            )),
        ),
    }
}

struct TcpEndpoint {
    host: String,
    port: u16,
}

fn parse_tcp_endpoint(raw_url: &str) -> anyhow::Result<TcpEndpoint> {
    let url = url::Url::parse(raw_url)?;
    let host = url
        .host_str()
        .ok_or_else(|| anyhow::anyhow!("URL has no host"))?
        .to_owned();
    let port = url
        .port_or_known_default()
        .ok_or_else(|| anyhow::anyhow!("URL has no port and no known default"))?;
    Ok(TcpEndpoint { host, port })
}

fn chain_observer_has_password(config: &SetupConfigView) -> bool {
    match &config.chain_observer.backend {
        ChainObserverBackendView::Esplora { .. } => false,
        ChainObserverBackendView::Bitcoind { has_password, .. } => *has_password,
    }
}

fn summary(checks: Vec<SetupValidationCheck>) -> SetupValidationSummary {
    let status = if checks
        .iter()
        .all(|check| check.status == ValidationStatus::Passed)
    {
        ValidationStatus::Passed
    } else {
        ValidationStatus::Failed
    };
    SetupValidationSummary { status, checks }
}

fn check(
    name: impl Into<String>,
    status: ValidationStatus,
    detail: Option<String>,
) -> SetupValidationCheck {
    SetupValidationCheck {
        name: name.into(),
        status,
        detail,
    }
}

fn status_from_validation(validation: &SetupValidationSummary) -> SetupStatus {
    if validation.status == ValidationStatus::Passed {
        SetupStatus::Ready
    } else {
        SetupStatus::PendingValidation
    }
}

fn missing_fields(stored: &StoredSetupState) -> Vec<String> {
    if stored.config.is_none() {
        vec!["setup_config".to_owned()]
    } else {
        Vec::new()
    }
}

fn parse_setup_status(value: &str) -> ServiceResult<SetupStatus> {
    value
        .parse()
        .map_err(|_| internal_error(format!("unknown persisted setup status {value:?}")))
}

#[cfg(test)]
#[path = "../tests/setup_store.rs"]
mod tests;
