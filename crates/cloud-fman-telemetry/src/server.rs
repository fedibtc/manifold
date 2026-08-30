use std::{
    collections::HashMap,
    error::Error,
    future::Future,
    net::{IpAddr, SocketAddr},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use axum::{
    Json, Router,
    body::to_bytes,
    extract::{ConnectInfo, Request, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use fedi_decentralized_domain::ProtocolV1;
use fedi_decentralized_manifold_environment::MANIFOLD_ENVIRONMENT_PROFILE_REVISION;
use fedi_decentralized_peer_badge_verifier::PeerBadgeVerifier;
use fedi_decentralized_service_fleet_manager::{
    GuardianTelemetryRegistrationRequest, GuardianTelemetryRegistrationResponse,
    MAX_GUARDIAN_TELEMETRY_REGISTRATION_BYTES,
};
use fedi_iroh_rpc::iroh::EndpointId;
use fedi_iroh_rpc::iroh::{Endpoint, endpoint::presets};

use crate::{
    admission::display_name,
    archive::JournalArchive,
    auth,
    cipher::SecretCipher,
    config::{Args, MetricsRuntimeConfig},
    data_root_lock::DataRootLock,
    journal_poller::JournalPoller,
    journal_types::{ReceptionDay, unix_seconds},
    metrics_observability::MetricsObservability,
    metrics_policy::MetricsPolicy,
    metrics_poller::MetricsPoller,
    metrics_snapshot::render_metrics,
    store::{MetricExpositionView, Store, TargetMaterial},
};

const REGISTRATION_PATH: &str = "/v1/telemetry/registrations";

#[cfg(feature = "test-support")]
/// Build the real registration router around a concrete explicit-test verifier.
///
/// This exists only for defe-backed component composition tests.
pub async fn registration_router_for_test(
    data_dir: &std::path::Path,
    expected_origin: &str,
    verifier: PeerBadgeVerifier,
) -> Result<Router, Box<dyn Error>> {
    let data_lock = Arc::new(DataRootLock::acquire(data_dir)?);
    let store = Store::open(
        &data_dir.join("state.sqlite"),
        "explicit-test",
        SecretCipher::new(&[7; 32]),
        "explicit-test".into(),
        120,
    )
    .await?;
    let state = AppState {
        _data_lock: data_lock,
        store,
        verifier,
        expected_url: format!("{expected_origin}{REGISTRATION_PATH}"),
        admission: Arc::new(tokio::sync::Semaphore::new(16)),
        source_budget: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
        source_budget_per_minute: 4,
        trusted_proxies: Arc::new(Vec::new()),
        metrics: MetricsRuntimeConfig {
            cadence: std::time::Duration::from_secs(1800),
            concurrency: std::num::NonZeroUsize::new(1).unwrap(),
            stale_after: std::time::Duration::from_secs(3600),
            source_version_requirement: "*".into(),
            source_version_hash: "test".into(),
            canonical_method_labels: false,
        },
        metrics_scrape: Arc::new(tokio::sync::Semaphore::new(1)),
        metrics_generation: Arc::new(tokio::sync::Semaphore::new(1)),
        metrics_cache: Arc::new(tokio::sync::Mutex::new(None)),
        metrics_observability: MetricsObservability::default(),
    };
    Ok(public_router(state).layer(axum::Extension(ConnectInfo(
        "127.0.0.1:1234".parse::<SocketAddr>()?,
    ))))
}

#[derive(Clone)]
struct AppState {
    /// Every accepted connection clones state, retaining exclusivity until that connection ends.
    _data_lock: Arc<DataRootLock>,
    store: Store,
    verifier: PeerBadgeVerifier,
    expected_url: String,
    admission: Arc<tokio::sync::Semaphore>,
    source_budget: Arc<tokio::sync::Mutex<HashMap<String, (i64, u8)>>>,
    /// Registrations admitted per source network prefix each minute.
    source_budget_per_minute: u8,
    trusted_proxies: Arc<Vec<ipnet::IpNet>>,
    metrics: MetricsRuntimeConfig,
    metrics_scrape: Arc<tokio::sync::Semaphore>,
    metrics_generation: Arc<tokio::sync::Semaphore>,
    metrics_cache: Arc<tokio::sync::Mutex<Option<MetricsCacheEntry>>>,
    metrics_observability: MetricsObservability,
}

struct MetricsCacheEntry {
    revision: i64,
    next_lease_expiry: Option<i64>,
    freshness_bucket: i64,
    body: axum::body::Bytes,
}

struct MetricsBodyBacking {
    body: String,
    _generation: tokio::sync::OwnedSemaphorePermit,
}

impl AsRef<[u8]> for MetricsBodyBacking {
    fn as_ref(&self) -> &[u8] {
        self.body.as_bytes()
    }
}

/// Start the public/private listeners under one exclusive data-root lock.
pub async fn serve(args: Args) -> Result<(), Box<dyn Error>> {
    let crate::config::RuntimeConfig {
        environment,
        metrics: metrics_runtime,
        log_cadence,
        log_concurrency,
        log_quota_bytes,
        log_retention_days,
        source_budget,
        #[cfg(feature = "defe-test-support")]
        e2e_iroh_endpoint_addr,
        #[cfg(feature = "defe-test-support")]
        e2e_poll_cadence,
        #[cfg(feature = "defe-test-support")]
        e2e_badge_verifier,
    } = args.validate()?;
    #[cfg(not(feature = "defe-test-support"))]
    let e2e_iroh_endpoint_addr = None;
    #[cfg(not(feature = "defe-test-support"))]
    let e2e_poll_cadence = None;
    #[cfg(not(feature = "defe-test-support"))]
    let e2e_badge_verifier = None;
    let data_lock = Arc::new(DataRootLock::acquire(&args.data_dir)?);
    // The supervisor retains exclusivity until every worker has joined. HTTP state
    // has a separate clone so listener teardown cannot release the process lock.
    let _supervisor_data_lock = data_lock.clone();
    let key = read_key(&args.key_file)?;
    let store = Store::open(
        &args.data_dir.join("state.sqlite"),
        &format!("{environment}:profile-{MANIFOLD_ENVIRONMENT_PROFILE_REVISION}"),
        SecretCipher::new(&key),
        args.key_id.clone(),
        args.lease_seconds,
    )
    .await?;
    let policy = MetricsPolicy {
        version_requirement: &metrics_runtime.source_version_requirement,
        version_hash: &metrics_runtime.source_version_hash,
        canonical_method_labels: metrics_runtime.canonical_method_labels,
    };
    store
        .configure_metrics_policy(&policy.fingerprint())
        .await?;
    let archive = JournalArchive::open(&args.data_dir, log_quota_bytes.get())?;
    let now = unix_seconds()?;
    let retention_seconds = i64::from(log_retention_days.get() - 1)
        .checked_mul(86_400)
        .ok_or("log retention is out of range")?;
    let cutoff = ReceptionDay::from_unix_seconds(
        now.checked_sub(retention_seconds)
            .ok_or("log retention is out of range")?,
    )?;
    store.prune_archive_ledger(&cutoff).await?;
    archive.recover(store.final_frame_boundaries().await?)?;
    let collector_endpoint = if e2e_iroh_endpoint_addr.is_some() {
        Endpoint::builder(presets::N0DisableRelay).bind().await?
    } else {
        Endpoint::builder(presets::N0).bind().await?
    };
    let journal_poller = JournalPoller::new(
        store.clone(),
        archive,
        collector_endpoint.clone(),
        log_concurrency,
        log_retention_days,
        cutoff,
        e2e_iroh_endpoint_addr.clone(),
    );
    let metrics_poller = MetricsPoller::new(
        store.clone(),
        collector_endpoint,
        metrics_runtime.source_version_requirement.clone(),
        metrics_runtime.source_version_hash.clone(),
        metrics_runtime.canonical_method_labels,
        metrics_runtime.concurrency,
        e2e_poll_cadence.unwrap_or(metrics_runtime.cadence),
    )
    .with_address_override(e2e_iroh_endpoint_addr);
    let metrics_observability = metrics_poller.observability();
    let verifier = match e2e_badge_verifier {
        Some(verifier) => verifier,
        None => PeerBadgeVerifier::try_from_profile(&environment.profile()?)?,
    };
    let state = AppState {
        _data_lock: data_lock,
        store,
        verifier,
        expected_url: format!("{}{REGISTRATION_PATH}", args.public_base_url),
        admission: Arc::new(tokio::sync::Semaphore::new(16)),
        source_budget: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
        source_budget_per_minute: source_budget,
        trusted_proxies: Arc::new(args.trusted_proxies),
        metrics: metrics_runtime,
        metrics_scrape: Arc::new(tokio::sync::Semaphore::new(1)),
        metrics_generation: Arc::new(tokio::sync::Semaphore::new(1)),
        metrics_cache: Arc::new(tokio::sync::Mutex::new(None)),
        metrics_observability,
    };
    let public = public_router(state.clone());
    let private = private_router(state);
    let public_listener = tokio::net::TcpListener::bind(args.public_bind).await?;
    let private_listener = tokio::net::TcpListener::bind(args.private_bind).await?;
    let (shutdown_sender, shutdown_receiver) = tokio::sync::watch::channel(false);
    let public_shutdown = shutdown_receiver.clone();
    let private_shutdown = shutdown_receiver.clone();
    let journal_task = tokio::spawn(
        journal_poller.run(e2e_poll_cadence.unwrap_or(log_cadence), shutdown_receiver),
    );
    let metrics_task = tokio::spawn(metrics_poller.run(shutdown_sender.subscribe()));
    supervise(
        shutdown_sender,
        serve_listener(public_listener, public, public_shutdown),
        serve_listener(private_listener, private, private_shutdown),
        journal_task,
        metrics_task,
        shutdown_signal(),
    )
    .await
}

async fn supervise<Public, Private, Shutdown, JournalError, MetricsError>(
    shutdown_sender: tokio::sync::watch::Sender<bool>,
    public_server: Public,
    private_server: Private,
    mut journal_task: tokio::task::JoinHandle<Result<(), JournalError>>,
    mut metrics_task: tokio::task::JoinHandle<Result<(), MetricsError>>,
    shutdown: Shutdown,
) -> Result<(), Box<dyn Error>>
where
    Public: Future<Output = std::io::Result<()>>,
    Private: Future<Output = std::io::Result<()>>,
    Shutdown: Future<Output = ()>,
    JournalError: Error + 'static,
    MetricsError: Error + 'static,
{
    let mut public_server = Box::pin(public_server);
    let mut private_server = Box::pin(private_server);
    let mut shutdown = Box::pin(shutdown);
    let mut public_result = None;
    let mut private_result = None;
    let mut journal_result = None;
    let mut metrics_result = None;
    tokio::select! {
        result = &mut public_server => {
            public_result = Some(result);
        },
        result = &mut private_server => {
            private_result = Some(result);
        },
        _ = &mut shutdown => {},
        result = &mut journal_task => {
            journal_result = Some(result);
        },
        result = &mut metrics_task => {
            metrics_result = Some(result);
        },
    };
    shutdown_sender.send_replace(true);

    let listeners = tokio::time::timeout(std::time::Duration::from_secs(30), async {
        tokio::join!(
            async {
                match public_result {
                    Some(result) => result,
                    None => public_server.await,
                }
            },
            async {
                match private_result {
                    Some(result) => result,
                    None => private_server.await,
                }
            }
        )
    })
    .await;
    let (journal_result, metrics_result) = tokio::join!(
        async {
            match journal_result {
                Some(result) => result,
                None => journal_task.await,
            }
        },
        async {
            match metrics_result {
                Some(result) => result,
                None => metrics_task.await,
            }
        }
    );

    // Evaluate failures only after every sibling has reached a definite outcome.
    let (public_result, private_result) = listeners.map_err(|_| "listeners did not drain")?;
    journal_result.map_err(|_| "safe-journal worker failed")??;
    metrics_result.map_err(|_| "metrics worker failed")??;
    public_result?;
    private_result?;
    Ok(())
}

fn public_router(state: AppState) -> Router {
    Router::new()
        .route(REGISTRATION_PATH, post(register))
        .layer(tower_http::timeout::TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            std::time::Duration::from_secs(20),
        ))
        .with_state(state)
}

fn private_router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(|| async { StatusCode::NO_CONTENT }))
        .route("/ready", get(ready))
        .route("/metrics", get(metrics))
        .with_state(state)
}

async fn serve_listener(
    listener: tokio::net::TcpListener,
    app: Router,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) -> std::io::Result<()> {
    use hyper_util::{rt::TokioIo, service::TowerToHyperService};
    let permits = Arc::new(tokio::sync::Semaphore::new(64));
    let mut connections = tokio::task::JoinSet::new();
    loop {
        tokio::select! {
            biased;
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    break;
                }
            }
            Some(_) = connections.join_next(), if !connections.is_empty() => {}
            accepted = listener.accept() => {
                let (stream, peer) = accepted?;
                let Ok(permit) = permits.clone().try_acquire_owned() else {
                    continue;
                };
                let service = app.clone().layer(axum::Extension(ConnectInfo(peer)));
                connections.spawn(async move {
                    let _permit = permit;
                    let connection = hyper::server::conn::http1::Builder::new()
                        .serve_connection(
                            TokioIo::new(stream),
                            TowerToHyperService::new(service),
                        );
                    let _ = tokio::time::timeout(
                        std::time::Duration::from_secs(25),
                        connection,
                    ).await;
                });
            }
        }
    }
    connections.shutdown().await;
    Ok(())
}

async fn register(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    request: Request,
) -> Response {
    match register_inner(&state, peer, request).await {
        Ok(()) => Json(GuardianTelemetryRegistrationResponse {
            version: ProtocolV1,
        })
        .into_response(),
        Err(error) => {
            let status = match error {
                RegisterError::TooLarge => StatusCode::PAYLOAD_TOO_LARGE,
                RegisterError::Refused => StatusCode::FORBIDDEN,
                RegisterError::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
            };
            (
                status,
                Json(serde_json::json!({"error":"registration_refused"})),
            )
                .into_response()
        }
    }
}

async fn register_inner(
    state: &AppState,
    peer: SocketAddr,
    request: Request,
) -> Result<(), RegisterError> {
    let authorization = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .ok_or(RegisterError::Refused)?
        .to_owned();
    let source = effective_source(peer.ip(), request.headers(), &state.trusted_proxies);
    let body = to_bytes(
        request.into_body(),
        MAX_GUARDIAN_TELEMETRY_REGISTRATION_BYTES,
    )
    .await
    .map_err(|_| RegisterError::TooLarge)?;
    let now = now();
    let auth = auth::verify(
        &authorization,
        &axum::http::Method::POST,
        &state.expected_url,
        &body,
        now,
    )
    .map_err(|_| RegisterError::Refused)?;
    let registration: GuardianTelemetryRegistrationRequest =
        serde_json::from_slice(&body).map_err(|_| RegisterError::Refused)?;
    let _ = registration.version;
    registration
        .iroh_endpoint_id
        .parse::<EndpointId>()
        .map_err(|_| RegisterError::Refused)?;
    enforce_source_budget(state, source, now).await?;
    let _permit = state
        .admission
        .clone()
        .try_acquire_owned()
        .map_err(|_| RegisterError::Unavailable)?;
    state
        .store
        .reserve_auth(&auth, now)
        .await
        .map_err(map_store_error)?;
    let badge = state
        .verifier
        .verify(&registration.holder_authorization)
        .await
        .map_err(|_| RegisterError::Refused)?;
    let subject = badge.subject().0.to_string();
    if subject != auth.signer {
        return Err(RegisterError::Refused);
    }
    let fman_name = display_name(&auth.signer).map_err(|_| RegisterError::Refused)?;
    state
        .store
        .admit(
            &auth,
            TargetMaterial {
                fman_pubkey: &auth.signer,
                fman_name: &fman_name,
                endpoint_id: &registration.iroh_endpoint_id,
                capability: registration.capability.as_bytes(),
                generation: registration.generation,
            },
            now,
        )
        .await
        .map_err(map_store_error)?;
    Ok(())
}

async fn enforce_source_budget(
    state: &AppState,
    source: IpAddr,
    now: i64,
) -> Result<(), RegisterError> {
    let mut budget = state.source_budget.lock().await;
    budget.retain(|_, (window, _)| *window > now - 60);
    let source = source_prefix(source);
    if !budget.contains_key(&source) && budget.len() >= 4096 {
        return Err(RegisterError::Unavailable);
    }
    let entry = budget.entry(source).or_insert((now, 0));
    if entry.0 <= now - 60 {
        *entry = (now, 0);
    }
    if entry.1 >= state.source_budget_per_minute {
        return Err(RegisterError::Unavailable);
    }
    entry.1 += 1;
    Ok(())
}

fn source_prefix(source: IpAddr) -> String {
    match normalize_ip(source) {
        IpAddr::V4(address) => {
            let [a, b, c, _] = address.octets();
            format!("v4:{a}.{b}.{c}.0/24")
        }
        IpAddr::V6(address) => {
            let segments = address.segments();
            format!(
                "v6:{:x}:{:x}:{:x}:{:x}::/64",
                segments[0], segments[1], segments[2], segments[3]
            )
        }
    }
}

fn effective_source(
    direct: IpAddr,
    headers: &axum::http::HeaderMap,
    trusted_proxies: &[ipnet::IpNet],
) -> IpAddr {
    let direct = normalize_ip(direct);
    if !trusted_proxies
        .iter()
        .any(|network| network.contains(&direct))
    {
        return direct;
    }
    let mut chain = Vec::new();
    if let Some(value) = headers
        .get("forwarded")
        .and_then(|value| value.to_str().ok())
    {
        for item in value.split(',') {
            for parameter in item.split(';') {
                if let Some((name, value)) = parameter.trim().split_once('=')
                    && name.trim().eq_ignore_ascii_case("for")
                    && let Some(ip) = parse_forwarded_ip(value)
                {
                    chain.push(ip);
                }
            }
        }
    }
    if chain.is_empty()
        && let Some(value) = headers
            .get("x-forwarded-for")
            .and_then(|value| value.to_str().ok())
    {
        chain.extend(value.split(',').filter_map(parse_forwarded_ip));
    }
    chain
        .iter()
        .rev()
        .copied()
        .map(normalize_ip)
        .find(|ip| !trusted_proxies.iter().any(|network| network.contains(ip)))
        .or_else(|| chain.first().copied().map(normalize_ip))
        .unwrap_or(direct)
}

fn parse_forwarded_ip(value: &str) -> Option<IpAddr> {
    let value = value.trim_matches('"').trim();
    if let Ok(ip) = value.parse() {
        return Some(normalize_ip(ip));
    }
    if let Some(rest) = value.strip_prefix('[') {
        return rest.split(']').next()?.parse().ok().map(normalize_ip);
    }
    value
        .split(':')
        .next()?
        .trim()
        .parse()
        .ok()
        .map(normalize_ip)
}

fn normalize_ip(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(ip) => ip.to_ipv4_mapped().map_or(IpAddr::V6(ip), IpAddr::V4),
        ip => ip,
    }
}

fn map_store_error(error: crate::store::StoreError) -> RegisterError {
    if error.is_refusal() {
        RegisterError::Refused
    } else {
        RegisterError::Unavailable
    }
}

async fn ready(State(state): State<AppState>) -> StatusCode {
    if state.store.ready().await {
        StatusCode::NO_CONTENT
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    }
}

async fn metrics(State(state): State<AppState>) -> Response {
    metrics_at(state, unix_millis()).await
}

async fn metrics_at(state: AppState, now_ms: i64) -> Response {
    let Ok(_permit) = state.metrics_scrape.clone().try_acquire_owned() else {
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    };
    let mut cache = state.metrics_cache.lock().await;
    let freshness_bucket = now_ms / 30_000;
    let version = match state.store.metric_exposition_version(now_ms / 1000).await {
        Ok(version) => version,
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    if let Some(entry) = cache.as_ref()
        && (
            entry.revision,
            entry.next_lease_expiry,
            entry.freshness_bucket,
        ) == (
            version.revision,
            version.next_lease_expiry,
            freshness_bucket,
        )
    {
        return (
            [(
                header::CONTENT_TYPE,
                "text/plain; version=0.0.4; charset=utf-8",
            )],
            entry.body.clone(),
        )
            .into_response();
    }
    *cache = None;
    let Ok(generation) = state.metrics_generation.clone().try_acquire_owned() else {
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    };
    let stale_after_ms = i64::try_from(state.metrics.stale_after.as_millis()).unwrap_or(i64::MAX);
    let policy = MetricsPolicy {
        version_requirement: &state.metrics.source_version_requirement,
        version_hash: &state.metrics.source_version_hash,
        canonical_method_labels: state.metrics.canonical_method_labels,
    };
    let MetricExpositionView {
        version,
        snapshots,
        targets,
    } = match state
        .store
        .metric_exposition(&policy, now_ms / 1000, now_ms, stale_after_ms / 1000)
        .await
    {
        Ok(exposition) => exposition,
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    match render_metrics(
        snapshots.snapshots,
        targets,
        snapshots.rejected,
        now_ms,
        stale_after_ms,
        policy.method_source_ready(),
        &state.metrics_observability,
    ) {
        Ok(body) => {
            let body = axum::body::Bytes::from_owner(MetricsBodyBacking {
                body,
                _generation: generation,
            });
            *cache = Some(MetricsCacheEntry {
                revision: version.revision,
                next_lease_expiry: version.next_lease_expiry,
                freshness_bucket,
                body: body.clone(),
            });
            (
                [(
                    header::CONTENT_TYPE,
                    "text/plain; version=0.0.4; charset=utf-8",
                )],
                body,
            )
                .into_response()
        }
        Err(_) => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            i64::try_from(duration.as_secs()).unwrap_or(i64::MAX)
        })
}

fn unix_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
        })
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .expect("install SIGTERM handler");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = terminate.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

fn read_key(path: &std::path::Path) -> Result<[u8; 32], Box<dyn Error>> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        if std::fs::metadata(path)?.permissions().mode() & 0o077 != 0 {
            return Err("encryption key file must not be accessible by group or other".into());
        }
    }
    let bytes = std::fs::read(path)?;
    bytes
        .try_into()
        .map_err(|_| "encryption key file must contain exactly 32 raw bytes".into())
}

#[derive(Debug)]
enum RegisterError {
    Refused,
    TooLarge,
    Unavailable,
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{Engine as _, engine::general_purpose};
    use blind_rsa_signatures::Signature as PbrsaSignature;
    use fedi_credential_sdk_protocol::{
        Credential, CredentialDigest, CredentialProof, HolderAuthorization,
        HolderAuthorizationStatement, HolderId, IssuerId, SignedCredential, SubjectPubkey,
        Timestamp as CredentialTimestamp,
    };
    use fedi_decentralized_domain::{HolderAuthorizationEnvelope, SchnorrSignatureProof};
    use fedi_decentralized_manifold_environment::ManifoldEnvironment;
    use fedi_decentralized_service_fleet_manager::TelemetryCapability;
    use nostr::{EventBuilder, Keys, Kind, Tag, Timestamp, secp256k1::Message};
    use sha2::{Digest as _, Sha256};
    use tower::ServiceExt as _;

    fn holder_authorization(subject: &Keys) -> HolderAuthorizationEnvelope {
        let holder = Keys::parse(&format!("{:064x}", 0x7002_u64)).unwrap();
        let issuer = Keys::parse(&format!("{:064x}", 0x7003_u64)).unwrap();
        let credential = Credential {
            issuer_id_pubkey: IssuerId(issuer.public_key()),
            info: serde_json::json!({"schema": "fedi-trust-score-v1.0"}),
            blind_msg: serde_json::json!(holder.public_key().to_string()),
        };
        let statement = HolderAuthorizationStatement {
            holder_id_pubkey: HolderId(holder.public_key()),
            subject_pubkey: SubjectPubkey(subject.public_key()),
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

    fn authorization(keys: &Keys, url: &str, body: &[u8]) -> String {
        let payload = hex::encode(Sha256::digest(body));
        let event = EventBuilder::new(Kind::HttpAuth, "")
            .custom_created_at(Timestamp::from(u64::try_from(now()).unwrap()))
            .tag(Tag::parse(["u", url]).unwrap())
            .tag(Tag::parse(["method", "POST"]).unwrap())
            .tag(Tag::parse(["payload", &payload]).unwrap())
            .sign_with_keys(keys)
            .unwrap();
        format!(
            "Nostr {}",
            general_purpose::STANDARD.encode(serde_json::to_vec(&event).unwrap())
        )
    }

    async fn state(directory: &tempfile::TempDir) -> AppState {
        state_with_lease(directory, 120).await
    }

    async fn state_with_lease(directory: &tempfile::TempDir, lease_seconds: i64) -> AppState {
        let data_dir = directory.path().join("collector");
        let data_lock = Arc::new(DataRootLock::acquire(&data_dir).unwrap());
        let store = Store::open(
            &data_dir.join("state.sqlite"),
            "development",
            SecretCipher::new(&[7; 32]),
            "test".into(),
            lease_seconds,
        )
        .await
        .unwrap();
        AppState {
            _data_lock: data_lock,
            store,
            verifier: PeerBadgeVerifier::try_from_profile(
                &ManifoldEnvironment::Development.profile().unwrap(),
            )
            .unwrap(),
            expected_url: "https://collector.test/v1/telemetry/registrations".into(),
            admission: Arc::new(tokio::sync::Semaphore::new(1)),
            source_budget: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            source_budget_per_minute: 4,
            trusted_proxies: Arc::new(Vec::new()),
            metrics: MetricsRuntimeConfig {
                cadence: std::time::Duration::from_secs(1800),
                concurrency: std::num::NonZeroUsize::new(1).unwrap(),
                stale_after: std::time::Duration::from_secs(3600),
                source_version_requirement: "*".into(),
                source_version_hash: "test".into(),
                canonical_method_labels: false,
            },
            metrics_scrape: Arc::new(tokio::sync::Semaphore::new(1)),
            metrics_generation: Arc::new(tokio::sync::Semaphore::new(1)),
            metrics_cache: Arc::new(tokio::sync::Mutex::new(None)),
            metrics_observability: MetricsObservability::default(),
        }
    }

    fn request(keys: &Keys, body: Vec<u8>) -> Request {
        let authorization = authorization(
            keys,
            "https://collector.test/v1/telemetry/registrations",
            &body,
        );
        Request::builder()
            .method("POST")
            .uri(REGISTRATION_PATH)
            .header(header::AUTHORIZATION, authorization)
            .body(axum::body::Body::from(body))
            .unwrap()
    }

    fn body(keys: &Keys) -> Vec<u8> {
        serde_json::to_vec(&GuardianTelemetryRegistrationRequest {
            version: ProtocolV1,
            generation: 7,
            iroh_endpoint_id: fedi_iroh_rpc::iroh::SecretKey::from_bytes(&[8; 32])
                .public()
                .to_string(),
            capability: TelemetryCapability::from_bytes([9; 32]),
            holder_authorization: holder_authorization(keys),
        })
        .unwrap()
    }

    fn test_router(state: AppState) -> Router {
        Router::new()
            .route(REGISTRATION_PATH, post(register))
            .layer(axum::Extension(ConnectInfo(
                "127.0.0.1:1234".parse::<SocketAddr>().unwrap(),
            )))
            .with_state(state)
    }

    #[tokio::test]
    async fn router_rejects_unverified_registration_without_creating_target() {
        let keys = Keys::generate();
        let directory = tempfile::tempdir().unwrap();
        let refused_state = state(&directory).await;
        let response = test_router(refused_state.clone())
            .oneshot(request(&keys, body(&keys)))
            .await;
        let response = response.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert_eq!(
            refused_state
                .store
                .target_status(&keys.public_key().to_string(), now())
                .await
                .unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn router_enforces_method_and_body_boundaries() {
        let directory = tempfile::tempdir().unwrap();
        let state = state(&directory).await;
        let wrong_method = Request::builder()
            .method("GET")
            .uri(REGISTRATION_PATH)
            .body(axum::body::Body::empty())
            .unwrap();
        assert_eq!(
            test_router(state.clone())
                .oneshot(wrong_method)
                .await
                .unwrap()
                .status(),
            StatusCode::METHOD_NOT_ALLOWED
        );

        let keys = Keys::generate();
        let oversized = vec![b'x'; MAX_GUARDIAN_TELEMETRY_REGISTRATION_BYTES + 1];
        assert_eq!(
            test_router(state)
                .oneshot(request(&keys, oversized))
                .await
                .unwrap()
                .status(),
            StatusCode::PAYLOAD_TOO_LARGE
        );
    }

    #[tokio::test]
    async fn private_routes_keep_local_readiness_independent_of_remote_snapshots() {
        use tower::ServiceExt as _;

        let directory = tempfile::tempdir().unwrap();
        let app = private_router(state(&directory).await);
        let ready_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/ready")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(ready_response.status(), StatusCode::NO_CONTENT);
        let metrics_response = app
            .oneshot(
                Request::builder()
                    .uri("/metrics")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(metrics_response.status(), StatusCode::OK);
        assert_eq!(
            metrics_response.headers()[header::CONTENT_TYPE],
            "text/plain; version=0.0.4; charset=utf-8"
        );
    }

    #[tokio::test]
    async fn private_metrics_allows_only_one_aggregate_render() {
        use tower::ServiceExt as _;

        let directory = tempfile::tempdir().unwrap();
        let state = state(&directory).await;
        let permit = state.metrics_scrape.clone().acquire_owned().await.unwrap();
        let response = private_router(state)
            .oneshot(
                Request::builder()
                    .uri("/metrics")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        drop(permit);
    }

    #[tokio::test]
    async fn slow_response_and_followup_share_one_cached_allocation() {
        use tower::ServiceExt as _;

        let directory = tempfile::tempdir().unwrap();
        let state = state(&directory).await;
        let app = private_router(state.clone());
        let slow_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/metrics")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let first_ptr = state
            .metrics_cache
            .lock()
            .await
            .as_ref()
            .unwrap()
            .body
            .as_ptr() as usize;
        let followup = app
            .oneshot(
                Request::builder()
                    .uri("/metrics")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let second_ptr = state
            .metrics_cache
            .lock()
            .await
            .as_ref()
            .unwrap()
            .body
            .as_ptr() as usize;
        assert_eq!(slow_response.status(), StatusCode::OK);
        assert_eq!(followup.status(), StatusCode::OK);
        assert_eq!(first_ptr, second_ptr);
        drop(slow_response);
    }

    #[tokio::test]
    async fn slow_reader_bounds_all_subsequent_revision_generations() {
        use tower::ServiceExt as _;

        let directory = tempfile::tempdir().unwrap();
        let state = state(&directory).await;
        let app = private_router(state.clone());
        let slow_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/metrics")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(slow_response.status(), StatusCode::OK);

        let observed_at = now();
        let auth = crate::auth::VerifiedHttpAuth {
            signer: "11".repeat(32),
            event_id: "metrics-generation".into(),
            created_at: observed_at,
        };
        state.store.reserve_auth(&auth, observed_at).await.unwrap();
        state
            .store
            .admit(
                &auth,
                TargetMaterial {
                    fman_pubkey: &auth.signer,
                    fman_name: "same-display",
                    endpoint_id: "endpoint",
                    capability: &[7; 32],
                    generation: 1,
                },
                observed_at,
            )
            .await
            .unwrap();
        for quarantined in [true, false, true, false] {
            if quarantined {
                state.store.quarantine(&auth.signer).await.unwrap();
            } else {
                state.store.reactivate(&auth.signer).await.unwrap();
            }
            let blocked = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri("/metrics")
                        .body(axum::body::Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(blocked.status(), StatusCode::TOO_MANY_REQUESTS);
        }
        drop(slow_response);
        let refreshed = app
            .oneshot(
                Request::builder()
                    .uri("/metrics")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(refreshed.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn no_snapshot_target_disappears_from_cache_at_lease_expiry() {
        let directory = tempfile::tempdir().unwrap();
        let signer = "11".repeat(32);
        let state = state_with_lease(&directory, 1).await;
        let observed_at = 100;
        let auth = crate::auth::VerifiedHttpAuth {
            signer: signer.clone(),
            event_id: "metrics-expiry-cache".into(),
            created_at: observed_at,
        };
        state.store.reserve_auth(&auth, observed_at).await.unwrap();
        state
            .store
            .admit(
                &auth,
                TargetMaterial {
                    fman_pubkey: &signer,
                    fman_name: "same-display",
                    endpoint_id: "endpoint",
                    capability: &[7; 32],
                    generation: 1,
                },
                observed_at,
            )
            .await
            .unwrap();
        let active = metrics_at(state.clone(), observed_at * 1000).await;
        let active = to_bytes(active.into_body(), 1024 * 1024).await.unwrap();
        assert!(String::from_utf8_lossy(&active).contains(&signer));
        drop(active);

        let expired = metrics_at(state, (observed_at + 1) * 1000).await;
        let expired = to_bytes(expired.into_body(), 1024 * 1024).await.unwrap();
        assert!(!String::from_utf8_lossy(&expired).contains(&signer));
    }

    #[tokio::test]
    async fn partial_remote_poll_is_degraded_while_local_readiness_stays_ready() {
        use tower::ServiceExt as _;

        let directory = tempfile::tempdir().unwrap();
        let state = state(&directory).await;
        let observed_at = now();
        let auth = crate::auth::VerifiedHttpAuth {
            signer: "11".repeat(32),
            event_id: "metrics-health".into(),
            created_at: observed_at,
        };
        state.store.reserve_auth(&auth, observed_at).await.unwrap();
        state
            .store
            .admit(
                &auth,
                TargetMaterial {
                    fman_pubkey: &auth.signer,
                    fman_name: "same-display",
                    endpoint_id: "endpoint",
                    capability: &[7; 32],
                    generation: 1,
                },
                observed_at,
            )
            .await
            .unwrap();
        let scheduled = state
            .store
            .due_metric_targets(observed_at)
            .await
            .unwrap()
            .remove(0);
        let work = state
            .store
            .begin_metric_work(&scheduled, observed_at, 1800)
            .await
            .unwrap()
            .unwrap();
        state
            .store
            .commit_metrics(
                &work,
                crate::metrics_types::MetricsCommit {
                    listed_seats: Some(Default::default()),
                    snapshots: Vec::new(),
                    complete: false,
                },
                observed_at,
            )
            .await
            .unwrap();
        let app = private_router(state.clone());
        assert_eq!(
            app.clone()
                .oneshot(
                    Request::builder()
                        .uri("/ready")
                        .body(axum::body::Body::empty())
                        .unwrap()
                )
                .await
                .unwrap()
                .status(),
            StatusCode::NO_CONTENT
        );
        let degraded = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/metrics")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let degraded = to_bytes(degraded.into_body(), 1024 * 1024).await.unwrap();
        assert!(String::from_utf8_lossy(&degraded).contains("target_fresh{"));
        assert!(String::from_utf8_lossy(&degraded).contains("} 0 "));
        drop(degraded);

        state
            .store
            .commit_metrics(
                &work,
                crate::metrics_types::MetricsCommit {
                    listed_seats: Some(Default::default()),
                    snapshots: Vec::new(),
                    complete: true,
                },
                observed_at,
            )
            .await
            .unwrap();
        let fresh = app
            .oneshot(
                Request::builder()
                    .uri("/metrics")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let fresh = to_bytes(fresh.into_body(), 1024 * 1024).await.unwrap();
        assert!(String::from_utf8_lossy(&fresh).contains("} 1 "));
    }

    #[tokio::test]
    async fn source_budget_is_bounded_by_network_prefix() {
        let directory = tempfile::tempdir().unwrap();
        let state = state(&directory).await;
        for host in 1..=4 {
            enforce_source_budget(&state, format!("192.0.2.{host}").parse().unwrap(), 100)
                .await
                .unwrap();
        }
        assert!(matches!(
            enforce_source_budget(&state, "192.0.2.99".parse().unwrap(), 100).await,
            Err(RegisterError::Unavailable)
        ));
        assert_eq!(state.source_budget.lock().await.len(), 1);
    }

    #[tokio::test]
    async fn a_raised_source_budget_admits_a_co_located_fleet() {
        let directory = tempfile::tempdir().unwrap();
        let mut state = state(&directory).await;
        state.source_budget_per_minute = 12;
        // One egress address, a whole fleet reconciling in the same second.
        for _ in 0..12 {
            enforce_source_budget(&state, "192.0.2.7".parse().unwrap(), 100)
                .await
                .unwrap();
        }
        assert!(matches!(
            enforce_source_budget(&state, "192.0.2.7".parse().unwrap(), 100).await,
            Err(RegisterError::Unavailable)
        ));
    }

    #[test]
    fn forwarded_source_is_used_only_from_a_trusted_proxy() {
        let headers = axum::http::HeaderMap::from_iter([(
            "x-forwarded-for".parse().unwrap(),
            "198.51.100.4, 10.0.0.8".parse().unwrap(),
        )]);
        let proxies = vec!["10.0.0.0/8".parse().unwrap()];
        assert_eq!(
            effective_source("10.0.0.9".parse().unwrap(), &headers, &proxies),
            "198.51.100.4".parse::<IpAddr>().unwrap()
        );
        assert_eq!(
            source_prefix("::ffff:192.0.2.9".parse().unwrap()),
            source_prefix("192.0.2.9".parse().unwrap())
        );
        let mapped_headers = axum::http::HeaderMap::from_iter([(
            "forwarded".parse().unwrap(),
            "for=\"[::ffff:198.51.100.4]\"".parse().unwrap(),
        )]);
        assert_eq!(
            effective_source("10.0.0.9".parse().unwrap(), &mapped_headers, &proxies),
            "198.51.100.4".parse::<IpAddr>().unwrap()
        );
        assert_eq!(
            effective_source("192.0.2.9".parse().unwrap(), &headers, &proxies),
            "192.0.2.9".parse::<IpAddr>().unwrap()
        );
    }

    async fn listener_shutdown(mut shutdown: tokio::sync::watch::Receiver<bool>) {
        if !*shutdown.borrow() {
            shutdown.changed().await.unwrap();
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn fatal_listener_joins_both_started_durability_workers() {
        let (shutdown_sender, shutdown_receiver) = tokio::sync::watch::channel(false);
        let entered = Arc::new(tokio::sync::Barrier::new(3));
        let release = Arc::new(tokio::sync::Barrier::new(3));
        let journal_done = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let metrics_done = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let worker = |done: Arc<std::sync::atomic::AtomicBool>| {
            let entered = entered.clone();
            let release = release.clone();
            tokio::spawn(async move {
                entered.wait().await;
                release.wait().await;
                done.store(true, std::sync::atomic::Ordering::SeqCst);
                Ok::<_, std::io::Error>(())
            })
        };
        let journal = worker(journal_done.clone());
        let metrics = worker(metrics_done.clone());
        let private_shutdown = shutdown_receiver.clone();
        let supervisor = supervise(
            shutdown_sender,
            async { Err(std::io::Error::other("fatal listener")) },
            async move {
                listener_shutdown(private_shutdown).await;
                Ok(())
            },
            journal,
            metrics,
            std::future::pending(),
        );
        let driver = async {
            entered.wait().await;
            tokio::task::yield_now().await;
            assert!(!journal_done.load(std::sync::atomic::Ordering::SeqCst));
            assert!(!metrics_done.load(std::sync::atomic::Ordering::SeqCst));
            release.wait().await;
        };
        let (result, ()) = tokio::join!(supervisor, driver);

        assert!(result.is_err());
        assert!(journal_done.load(std::sync::atomic::Ordering::SeqCst));
        assert!(metrics_done.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn shutdown_signal_joins_both_started_durability_workers() {
        let (shutdown_sender, shutdown_receiver) = tokio::sync::watch::channel(false);
        let entered = Arc::new(tokio::sync::Barrier::new(3));
        let release = Arc::new(tokio::sync::Barrier::new(3));
        let journal_done = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let metrics_done = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let worker = |done: Arc<std::sync::atomic::AtomicBool>| {
            let entered = entered.clone();
            let release = release.clone();
            tokio::spawn(async move {
                entered.wait().await;
                release.wait().await;
                done.store(true, std::sync::atomic::Ordering::SeqCst);
                Ok::<_, std::io::Error>(())
            })
        };
        let journal = worker(journal_done.clone());
        let metrics = worker(metrics_done.clone());
        let public_shutdown = shutdown_receiver.clone();
        let private_shutdown = shutdown_receiver;
        let (signal, signal_receiver) = tokio::sync::oneshot::channel();
        let supervisor = supervise(
            shutdown_sender,
            async move {
                listener_shutdown(public_shutdown).await;
                Ok(())
            },
            async move {
                listener_shutdown(private_shutdown).await;
                Ok(())
            },
            journal,
            metrics,
            async move {
                signal_receiver.await.unwrap();
            },
        );
        let driver = async {
            entered.wait().await;
            signal.send(()).unwrap();
            tokio::task::yield_now().await;
            assert!(!journal_done.load(std::sync::atomic::Ordering::SeqCst));
            assert!(!metrics_done.load(std::sync::atomic::Ordering::SeqCst));
            release.wait().await;
        };
        let (result, ()) = tokio::join!(supervisor, driver);

        result.unwrap();
        assert!(journal_done.load(std::sync::atomic::Ordering::SeqCst));
        assert!(metrics_done.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[test]
    fn supervisor_lock_outlives_listener_state_teardown() {
        use std::os::unix::fs::PermissionsExt as _;

        let directory = tempfile::tempdir().unwrap();
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let acquired = Arc::new(DataRootLock::acquire(directory.path()).unwrap());
        let supervisor = acquired.clone();
        let listener_state = acquired.clone();
        drop(acquired);
        drop(listener_state);
        assert!(DataRootLock::acquire(directory.path()).is_err());
        drop(supervisor);
        assert!(DataRootLock::acquire(directory.path()).is_ok());
    }
}
