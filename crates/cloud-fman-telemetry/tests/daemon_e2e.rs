//! Real-daemon Defe validation of the release's critical collection callpath.

use std::{
    io::Cursor,
    path::{Path, PathBuf},
    str::FromStr as _,
    time::Duration,
};

use base64::{Engine as _, engine::general_purpose};
use defe_api::{ResourceDescriptor, SharingMode};
use defe_client::AsyncDefeClient;
use fedi_credential_sdk_protocol::{
    HolderAuthorizationRequest, HolderContext, IssuerContext, IssuerSecretKeys, PendingIssuance,
    RevocationLocation, SubjectPubkey,
};
use fedi_decentralized_domain::{HolderAuthorizationEnvelope, ProtocolV1};
use fedi_decentralized_manifold_environment::ManifoldEnvironment;
use fedi_decentralized_nostr::attester::{
    ISSUER_AUTHORITY_D_TAG, ISSUER_AUTHORITY_EVENT_KIND, ISSUER_AUTHORITY_HASHTAG,
};
use fedi_decentralized_service_fleet_manager::{
    FetchSafeEventJournalRequest, FetchSafeEventJournalResponse, GUARDIAN_TELEMETRY_ALPN,
    GuardianMetricsResponse, GuardianTelemetryApi, GuardianTelemetryApiServer,
    GuardianTelemetryRegistrationRequest, GuardianTelemetrySeat, ListGuardianTelemetrySeatsRequest,
    ListGuardianTelemetrySeatsResponse, ListSafeEventJournalsRequest,
    ListSafeEventJournalsResponse, SafeEventCursor, SafeEventJournal, SafeEventJournalIncarnation,
    SafeEventJournalInfo, ScrapeGuardianMetricsRequest, SeatId, TelemetryCapability,
    TelemetryResult,
};
use fedi_iroh_rpc::{
    IrohProtocol,
    iroh::{Endpoint, RelayMode, endpoint::presets, protocol::Router},
};
use nostr::{EventBuilder, Keys, Kind, Tag, Timestamp};
use sha2::{Digest as _, Sha256};
use tokio::{
    io::{AsyncReadExt as _, AsyncWriteExt as _},
    process::{Child, Command},
};

const VALID_INVITE: &str = "fed11qgqpu8rhwden5te0vejkg6tdd9h8gepwd4cxcumxv4jzuen0duhsqqfqh6nl7sgk72caxfx8khtfnn8y436q3nhyrkev3qp8ugdhdllnh86qmp42pm";

const CAPABILITY: [u8; 32] = [7; 32];
const JSONL: &[u8] = b"{\"fields\":{\"message\":\"safe fixture\",\"safe_to_share\":true}}\n";

#[derive(Clone)]
struct TelemetryFixture {
    incarnation: SafeEventJournalIncarnation,
    seat: SeatId,
}

impl GuardianTelemetryApi for TelemetryFixture {
    async fn list_guardian_telemetry_seats(
        &self,
        request: ListGuardianTelemetrySeatsRequest,
    ) -> TelemetryResult<ListGuardianTelemetrySeatsResponse> {
        assert_eq!(request.capability.as_bytes(), &CAPABILITY);
        Ok(ListGuardianTelemetrySeatsResponse {
            seats: vec![GuardianTelemetrySeat {
                seat_id: self.seat.clone(),
                invite_code: Some(fedi_decentralized_service_fleet_manager::InviteCode(
                    VALID_INVITE.to_owned(),
                )),
            }],
        })
    }

    async fn scrape_guardian_metrics(
        &self,
        request: ScrapeGuardianMetricsRequest,
    ) -> TelemetryResult<GuardianMetricsResponse> {
        assert_eq!(request.capability.as_bytes(), &CAPABILITY);
        assert_eq!(request.seat_id, self.seat);
        Ok(GuardianMetricsResponse {
            status_code: 200,
            content_type: Some("text/plain; version=0.0.4".to_owned()),
            content_encoding: None,
            body: b"fm_app_start_ts{version=\"test\",version_hash=\"hash\"} 1\nfm_consensus_session_count 2\n".to_vec(),
        })
    }

    async fn list_safe_event_journals(
        &self,
        request: ListSafeEventJournalsRequest,
    ) -> TelemetryResult<ListSafeEventJournalsResponse> {
        assert_eq!(request.capability.as_bytes(), &CAPABILITY);
        Ok(ListSafeEventJournalsResponse {
            journals: vec![SafeEventJournalInfo {
                journal: SafeEventJournal::Fman,
                incarnation: self.incarnation.clone(),
            }],
        })
    }

    async fn fetch_safe_event_journal(
        &self,
        request: FetchSafeEventJournalRequest,
    ) -> TelemetryResult<FetchSafeEventJournalResponse> {
        assert_eq!(request.capability.as_bytes(), &CAPABILITY);
        Ok(FetchSafeEventJournalResponse::Current {
            incarnation: self.incarnation.clone(),
            jsonl: if request.cursor.is_none() {
                JSONL.to_vec()
            } else {
                Vec::new()
            },
            next_cursor: Some(SafeEventCursor {
                incarnation: self.incarnation.clone(),
                segment: 1,
                offset: JSONL.len() as u64,
            }),
            continuity_gap: false,
        })
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn real_daemon_registers_pulls_persists_and_restarts() {
    let refused = Command::new(env!(
        "CARGO_BIN_EXE_fedi-decentralized-cloud-fman-telemetry"
    ))
    .env_remove("DEV_DEFE_SOCKET_PATH")
    .env(
        "CLOUD_FMAN_TELEMETRY_E2E_IROH_ENDPOINT_ADDR",
        "{\"id\":\"invalid\",\"addrs\":[]}",
    )
    .env("CLOUD_FMAN_TELEMETRY_E2E_POLL_MILLIS", "100")
    .env(
        "CLOUD_FMAN_TELEMETRY_PUBLIC_BASE_URL",
        "https://collector.test",
    )
    .env("CLOUD_FMAN_TELEMETRY_DATA_DIR", "/nonexistent")
    .env("CLOUD_FMAN_TELEMETRY_KEY_FILE", "/nonexistent")
    .env("CLOUD_FMAN_TELEMETRY_KEY_ID", "test")
    .env("CLOUD_FMAN_TELEMETRY_ENVIRONMENT", "development")
    .output()
    .await
    .expect("run fail-closed E2E configuration probe");
    assert!(!refused.status.success());
    assert!(
        String::from_utf8_lossy(&refused.stderr)
            .contains("E2E configuration is available only under Defe")
    );

    let mut defe = AsyncDefeClient::connect_from_env()
        .await
        .expect("test runs under defe");
    let lease = defe
        .request_nostr_relay(SharingMode::Exclusive)
        .await
        .expect("lease local relay");
    let ResourceDescriptor::NostrRelay(relay) = &lease.descriptor else {
        panic!("Defe returned the wrong resource");
    };

    let (issuer, issuer_id) = publish_test_authority(&relay.url).await;
    let fman_keys = Keys::generate();
    let authorization = issue_authorization(&issuer, &fman_keys);
    let endpoint = Endpoint::builder(presets::N0)
        .relay_mode(RelayMode::Disabled)
        .bind()
        .await
        .expect("bind local telemetry endpoint");
    let router = Router::builder(endpoint)
        .accept(
            GUARDIAN_TELEMETRY_ALPN,
            IrohProtocol::new(GuardianTelemetryApiServer::new(TelemetryFixture {
                incarnation: "018f22d0-4e5f-7abc-8def-0123456789ab".parse().unwrap(),
                seat: SeatId::new("22".repeat(32)).unwrap(),
            })),
        )
        .spawn();

    let data = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(data.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let key_file = data.path().join("key");
    std::fs::write(&key_file, [9_u8; 32]).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&key_file, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let public_port = defe_portalloc::port_alloc(2).expect("reserve collector port pair");
    let private_port = public_port.checked_add(1).unwrap();
    let mut daemon = start_daemon(
        data.path(),
        &key_file,
        public_port,
        private_port,
        &issuer_id,
        &relay.url,
        &serde_json::to_string(&router.endpoint().addr()).unwrap(),
    )
    .await;
    wait_for_status(private_port, "/ready", 204).await;

    let body = serde_json::to_vec(&GuardianTelemetryRegistrationRequest {
        version: ProtocolV1,
        iroh_endpoint_id: router.endpoint().id().to_string(),
        generation: 1,
        capability: TelemetryCapability::from_bytes(CAPABILITY),
        holder_authorization: authorization,
    })
    .unwrap();
    let auth = nip98(
        &fman_keys,
        "https://collector.test/v1/telemetry/registrations",
        &body,
    );
    let response = http(
        public_port,
        "POST",
        "/v1/telemetry/registrations",
        &[
            ("Authorization", &auth),
            ("Content-Type", "application/json"),
        ],
        &body,
    )
    .await;
    assert!(response.starts_with("HTTP/1.1 200"));

    let metrics = wait_for_metrics(private_port).await;
    assert!(metrics.contains("fm_consensus_session_count"));
    let federation_id = fedimint_core::invite_code::InviteCode::from_str(VALID_INVITE)
        .unwrap()
        .federation_id()
        .to_string();
    assert!(metrics.contains(&format!("federation_id=\"{federation_id}\"")));
    let target_fresh = metrics
        .lines()
        .find(|line| line.starts_with("cloud_fman_telemetry_target_fresh{"))
        .unwrap();
    assert!(!target_fresh.contains("federation_id"));
    let parsed = prometheus_parse::Scrape::parse(
        metrics
            .lines()
            .map(|line| Ok::<String, std::io::Error>(line.to_owned())),
    )
    .expect("Prometheus parser accepts exposition");
    assert!(!parsed.samples.is_empty());

    let archive = wait_for_archive(data.path()).await;
    assert_eq!(
        zstd::stream::decode_all(Cursor::new(std::fs::read(&archive).unwrap())).unwrap(),
        JSONL
    );

    terminate(&mut daemon).await;
    let before = std::fs::read(&archive).unwrap();
    {
        use std::io::Write as _;
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&archive)
            .unwrap();
        file.write_all(b"orphan-tail").unwrap();
        file.sync_data().unwrap();
    }
    let mut restarted = start_daemon(
        data.path(),
        &key_file,
        public_port,
        private_port,
        &issuer_id,
        &relay.url,
        &serde_json::to_string(&router.endpoint().addr()).unwrap(),
    )
    .await;
    wait_for_status(private_port, "/ready", 204).await;
    assert_eq!(
        std::fs::read(&archive).unwrap(),
        before,
        "startup truncates bytes beyond SQLite's committed frame"
    );
    assert!(
        wait_for_metrics(private_port)
            .await
            .contains("fm_consensus_session_count")
    );
    terminate(&mut restarted).await;

    router.shutdown().await.unwrap();
    defe.release(lease.handle_id).await.unwrap();
}

async fn publish_test_authority(relay: &str) -> (IssuerContext, String) {
    let secrets: IssuerSecretKeys = serde_json::from_str(
        ManifoldEnvironment::Development
            .profile()
            .unwrap()
            .test_issuer_secret_keys()
            .unwrap(),
    )
    .unwrap();
    let issuer = IssuerContext::import_secret_key(&secrets).unwrap();
    let authority = issuer
        .issuer_authority(vec![RevocationLocation {
            protocol: "nostr".to_owned(),
            location: relay.to_owned(),
        }])
        .unwrap();
    let metadata = authority.verify().unwrap();
    let keys = Keys::parse(&secrets.issuer_id_secret_key).unwrap();
    let client = nostr_sdk::Client::new(keys);
    client.add_relay(relay).await.unwrap();
    client.connect().await;
    let output = client
        .send_event_builder(
            EventBuilder::new(
                Kind::Custom(ISSUER_AUTHORITY_EVENT_KIND),
                serde_json::to_string(&authority).unwrap(),
            )
            .tags([
                Tag::identifier(ISSUER_AUTHORITY_D_TAG),
                Tag::hashtag(ISSUER_AUTHORITY_HASHTAG),
            ]),
        )
        .await
        .unwrap();
    assert!(!output.success.is_empty());
    client.disconnect().await;
    (issuer, metadata.issuer_id_pubkey.0.to_string())
}

fn issue_authorization(issuer: &IssuerContext, subject: &Keys) -> HolderAuthorizationEnvelope {
    let metadata = issuer
        .issuer_authority(Vec::new())
        .unwrap()
        .verify()
        .unwrap();
    let holder = HolderContext::generate();
    let (request, pending) = PendingIssuance::create_request(
        &metadata.issuance_key,
        metadata.issuer_id_pubkey,
        serde_json::json!({"schema":"fedi-trust-score-v1.0","trust_level":9}),
        serde_json::json!(holder.public_key().to_string()),
    )
    .unwrap();
    let response = issuer
        .issue_credential(pending.info.clone(), &request)
        .unwrap();
    let credential = pending.finalize(&metadata.issuance_key, &response).unwrap();
    let authorization = holder
        .authorize_credential_use(
            HolderAuthorizationRequest {
                subject_pubkey: subject
                    .public_key()
                    .to_string()
                    .parse::<SubjectPubkey>()
                    .unwrap(),
            },
            &credential,
        )
        .unwrap();
    HolderAuthorizationEnvelope {
        holder_authorization: authorization,
        signed_credential: credential,
    }
}

async fn start_daemon(
    data: &Path,
    key: &Path,
    public_port: u16,
    private_port: u16,
    issuer: &str,
    relay: &str,
    endpoint: &str,
) -> Child {
    let mut command = Command::new(env!(
        "CARGO_BIN_EXE_fedi-decentralized-cloud-fman-telemetry"
    ));
    command
        .env(
            "CLOUD_FMAN_TELEMETRY_PUBLIC_BASE_URL",
            "https://collector.test",
        )
        .env(
            "CLOUD_FMAN_TELEMETRY_PUBLIC_BIND",
            format!("127.0.0.1:{public_port}"),
        )
        .env(
            "CLOUD_FMAN_TELEMETRY_PRIVATE_BIND",
            format!("127.0.0.1:{private_port}"),
        )
        .env("CLOUD_FMAN_TELEMETRY_DATA_DIR", data)
        .env("CLOUD_FMAN_TELEMETRY_KEY_FILE", key)
        .env("CLOUD_FMAN_TELEMETRY_KEY_ID", "test-key")
        .env("CLOUD_FMAN_TELEMETRY_ENVIRONMENT", "development")
        .env("CLOUD_FMAN_TELEMETRY_E2E_IROH_ENDPOINT_ADDR", endpoint)
        .env("CLOUD_FMAN_TELEMETRY_E2E_POLL_MILLIS", "100")
        .env("CLOUD_FMAN_TELEMETRY_E2E_ISSUER", issuer)
        .env("CLOUD_FMAN_TELEMETRY_E2E_NOSTR_RELAY", relay)
        .env("RUST_LOG", "info")
        .kill_on_drop(true)
        .spawn()
        .expect("spawn real collector binary")
}

fn nip98(keys: &Keys, url: &str, body: &[u8]) -> String {
    let event = EventBuilder::new(Kind::HttpAuth, "")
        .custom_created_at(Timestamp::now())
        .tag(Tag::parse(["u", url]).unwrap())
        .tag(Tag::parse(["method", "POST"]).unwrap())
        .tag(Tag::parse(["payload", &hex::encode(Sha256::digest(body))]).unwrap())
        .sign_with_keys(keys)
        .unwrap();
    format!(
        "Nostr {}",
        general_purpose::STANDARD.encode(serde_json::to_vec(&event).unwrap())
    )
}

async fn wait_for_status(port: u16, path: &str, status: u16) {
    for _ in 0..150 {
        if http(port, "GET", path, &[], &[])
            .await
            .starts_with(&format!("HTTP/1.1 {status}"))
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("timed out waiting for {path}");
}

async fn wait_for_metrics(port: u16) -> String {
    for _ in 0..150 {
        let response = http(port, "GET", "/metrics", &[], &[]).await;
        if let Some((_, body)) = response.split_once("\r\n\r\n")
            && body.contains("fm_consensus_session_count")
        {
            return body.to_owned();
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("timed out waiting for collected metrics");
}

async fn wait_for_archive(root: &Path) -> PathBuf {
    for _ in 0..150 {
        if let Some(path) = files(root.join("logs"))
            .into_iter()
            .find(|path| path.extension().is_some_and(|extension| extension == "zst"))
            && std::fs::read(&path)
                .ok()
                .and_then(|bytes| zstd::stream::decode_all(Cursor::new(bytes)).ok())
                .is_some_and(|decoded| decoded == JSONL)
        {
            return path;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("timed out waiting for archive");
}

fn files(root: PathBuf) -> Vec<PathBuf> {
    let mut result = Vec::new();
    let Ok(entries) = std::fs::read_dir(root) else {
        return result;
    };
    for entry in entries.flatten() {
        if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            result.extend(files(entry.path()));
        } else {
            result.push(entry.path());
        }
    }
    result
}

async fn http(
    port: u16,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &[u8],
) -> String {
    let Ok(mut stream) = tokio::net::TcpStream::connect(("127.0.0.1", port)).await else {
        return String::new();
    };
    let mut request =
        format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n");
    for (name, value) in headers {
        request.push_str(&format!("{name}: {value}\r\n"));
    }
    request.push_str(&format!("Content-Length: {}\r\n\r\n", body.len()));
    if stream.write_all(request.as_bytes()).await.is_err() || stream.write_all(body).await.is_err()
    {
        return String::new();
    }
    let mut response = Vec::new();
    let _ = stream.read_to_end(&mut response).await;
    String::from_utf8_lossy(&response).into_owned()
}

async fn terminate(child: &mut Child) {
    let pid = i32::try_from(child.id().expect("child is running")).unwrap();
    assert_eq!(unsafe { libc::kill(pid, libc::SIGTERM) }, 0);
    let status = tokio::time::timeout(Duration::from_secs(10), child.wait())
        .await
        .expect("collector drains after SIGTERM")
        .unwrap();
    assert!(status.success());
}
