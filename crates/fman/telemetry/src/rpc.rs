//! Capability-scoped Iroh proxy for each child's loopback Prometheus endpoint.
//!
//! The child remains bound to `127.0.0.1`. This adapter authenticates a
//! receiver-registered FMan-wide bearer capability before selecting that
//! loopback port, then applies the compiled default-deny source policy before
//! constructing the Iroh response.

use std::sync::Arc;
use std::time::Duration;

use fedi_decentralized_service_fleet_manager::{
    FetchSafeEventJournalRequest, FetchSafeEventJournalResponse, GuardianMetricsResponse,
    GuardianTelemetryApi, ListGuardianTelemetrySeatsRequest, ListGuardianTelemetrySeatsResponse,
    ListSafeEventJournalsRequest, ListSafeEventJournalsResponse, MAX_GUARDIAN_METRICS_BODY_BYTES,
    MAX_SAFE_EVENT_BATCH_BYTES, SafeEventCursor, SafeEventJournalInfo,
    ScrapeGuardianMetricsRequest, ServiceError, ServiceErrorCode, TelemetryCapability,
    TelemetryResult,
};
use reqwest::header::{CONTENT_ENCODING, CONTENT_TYPE};

use fedi_decentralized_guardian_metrics_policy::MetricsPolicy;
use fman_core::fleet::{Fleet, TelemetryAccessError};

const UPSTREAM_CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
const UPSTREAM_TOTAL_TIMEOUT: Duration = Duration::from_secs(10);
const PROJECTION_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_SAFE_EVENT_RECORD_BYTES: usize = 64 * 1024;

/// Sanitizing guardian metrics proxy hosted on the dedicated telemetry Iroh ALPN.
#[derive(Clone)]
pub struct GuardianTelemetryRpc {
    fleet: Arc<Fleet>,
    http: reqwest::Client,
    max_body_bytes: usize,
}

impl GuardianTelemetryRpc {
    /// Build the production loopback-only client. Redirects are disabled so a
    /// compromised child cannot make the FMan fetch another local service.
    /// This client only ever fetches plain-HTTP loopback metrics, so it
    /// skips TLS root loading entirely: hosts without a usable system CA
    /// store (the CI sandbox) must not fail guardian startup over roots the
    /// proxy can never need.
    pub fn new(fleet: Arc<Fleet>) -> Result<Self, reqwest::Error> {
        let http = reqwest::Client::builder()
            .connect_timeout(UPSTREAM_CONNECT_TIMEOUT)
            .timeout(UPSTREAM_TOTAL_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .tls_built_in_root_certs(false)
            .build()?;
        Ok(Self {
            fleet,
            http,
            max_body_bytes: MAX_GUARDIAN_METRICS_BODY_BYTES,
        })
    }

    async fn proxy(
        &self,
        request: ScrapeGuardianMetricsRequest,
    ) -> TelemetryResult<GuardianMetricsResponse> {
        let metrics_port = match self
            .fleet
            .authorize_telemetry_scrape(&request.seat_id, &request.capability)
            .await
        {
            Ok(port) => port,
            Err(TelemetryAccessError::Unauthorized) => {
                return Err(ServiceError::with_code(
                    ServiceErrorCode::PermissionDenied,
                    "guardian telemetry access denied",
                ));
            }
            Err(TelemetryAccessError::Unavailable) => {
                return Err(ServiceError::with_code(
                    ServiceErrorCode::Unavailable,
                    "guardian metrics temporarily unavailable",
                ));
            }
        };

        fetch_guardian_metrics(&self.http, metrics_port, self.max_body_bytes).await
    }
}

/// Fetch and sanitize a validated seat response.
///
/// Split from authorization so byte, redirect, header, and projection behavior
/// can be tested without constructing a Fleet.
async fn fetch_guardian_metrics(
    http: &reqwest::Client,
    metrics_port: u16,
    max_body_bytes: usize,
) -> TelemetryResult<GuardianMetricsResponse> {
    // The address is constructed from a validated u16 port, not caller or
    // child input. It can never leave loopback or select another path.
    let url = format!("http://127.0.0.1:{metrics_port}/metrics");
    let mut upstream = http.get(url).send().await.map_err(|_| {
        tracing::warn!("guardian metrics loopback request failed");
        ServiceError::with_code(
            ServiceErrorCode::Unavailable,
            "guardian metrics temporarily unavailable",
        )
    })?;
    if upstream
        .content_length()
        .is_some_and(|length| length > max_body_bytes as u64)
    {
        tracing::warn!("guardian metrics response exceeded the byte limit");
        return Err(ServiceError::with_code(
            ServiceErrorCode::Unavailable,
            "guardian metrics response exceeded the byte limit",
        ));
    }

    if upstream.status().as_u16() != 200
        || upstream.headers().contains_key(CONTENT_ENCODING)
        || !header_text(upstream.headers().get(CONTENT_TYPE))
            .is_some_and(|value| value.starts_with("text/plain"))
    {
        tracing::warn!("guardian metrics response metadata was rejected");
        return Err(ServiceError::with_code(
            ServiceErrorCode::Unavailable,
            "guardian metrics temporarily unavailable",
        ));
    }
    let mut body = Vec::with_capacity(
        upstream
            .content_length()
            .and_then(|length| usize::try_from(length).ok())
            .unwrap_or(0),
    );
    while let Some(chunk) = upstream.chunk().await.map_err(|_| {
        tracing::warn!("guardian metrics response read failed");
        ServiceError::with_code(
            ServiceErrorCode::Unavailable,
            "guardian metrics response could not be read",
        )
    })? {
        if max_body_bytes.saturating_sub(body.len()) < chunk.len() {
            tracing::warn!("guardian metrics response exceeded the byte limit");
            return Err(ServiceError::with_code(
                ServiceErrorCode::Unavailable,
                "guardian metrics response exceeded the byte limit",
            ));
        }
        body.extend_from_slice(&chunk);
    }

    let projected = tokio::task::spawn_blocking(move || {
        MetricsPolicy.project_until(&body, Some(std::time::Instant::now() + PROJECTION_TIMEOUT))
    })
    .await
    .map_err(|_| {
        tracing::warn!("guardian metrics projection task failed");
        ServiceError::with_code(
            ServiceErrorCode::Internal,
            "guardian metrics internal error",
        )
    })?
    .map_err(|_| {
        tracing::warn!("guardian metrics response was rejected");
        ServiceError::with_code(
            ServiceErrorCode::Unavailable,
            "guardian metrics temporarily unavailable",
        )
    })?;
    let mut body = projected.samples.join("\n").into_bytes();
    if !body.is_empty() {
        body.push(b'\n');
    }
    Ok(GuardianMetricsResponse {
        status_code: 200,
        content_type: Some("text/plain; version=0.0.4".to_owned()),
        content_encoding: None,
        body,
    })
}

impl GuardianTelemetryApi for GuardianTelemetryRpc {
    async fn list_guardian_telemetry_seats(
        &self,
        request: ListGuardianTelemetrySeatsRequest,
    ) -> TelemetryResult<ListGuardianTelemetrySeatsResponse> {
        authorize_telemetry(&self.fleet, &request.capability)?;
        Ok(ListGuardianTelemetrySeatsResponse {
            seats: self.fleet.telemetry_seats(),
        })
    }

    async fn scrape_guardian_metrics(
        &self,
        request: ScrapeGuardianMetricsRequest,
    ) -> TelemetryResult<GuardianMetricsResponse> {
        self.proxy(request).await
    }

    async fn list_safe_event_journals(
        &self,
        request: ListSafeEventJournalsRequest,
    ) -> TelemetryResult<ListSafeEventJournalsResponse> {
        authorize_telemetry(&self.fleet, &request.capability)?;
        let directories = self
            .fleet
            .safe_event_journals()
            .into_iter()
            .filter_map(|journal| {
                self.fleet
                    .safe_event_journal_dir(&journal)
                    .map(|directory| (journal, directory))
            })
            .collect::<Vec<_>>();
        let journals = tokio::task::spawn_blocking(move || open_journal_infos(directories))
            .await
            .map_err(|_| {
                ServiceError::with_code(
                    ServiceErrorCode::Internal,
                    "safe-event journal internal error",
                )
            })?
            .map_err(|_| {
                ServiceError::with_code(
                    ServiceErrorCode::Unavailable,
                    "safe-event journals temporarily unavailable",
                )
            })?;
        Ok(ListSafeEventJournalsResponse { journals })
    }

    async fn fetch_safe_event_journal(
        &self,
        request: FetchSafeEventJournalRequest,
    ) -> TelemetryResult<FetchSafeEventJournalResponse> {
        authorize_telemetry(&self.fleet, &request.capability)?;
        let directory = self
            .fleet
            .safe_event_journal_dir(&request.journal)
            .ok_or_else(|| {
                ServiceError::with_code(
                    ServiceErrorCode::NotFound,
                    "safe-event journal was not found",
                )
            })?;
        let batch =
            tokio::task::spawn_blocking(move || fetch_journal_directory(directory, request))
                .await
                .map_err(|error| {
                    tracing::error!(error = %error, "safe-event journal reader task failed");
                    ServiceError::with_code(
                        ServiceErrorCode::Internal,
                        "safe-event journal internal error",
                    )
                })?
                .map_err(|error| {
                    tracing::warn!(error = %error, "safe-event journal read failed");
                    map_journal_read_error()
                })?;
        Ok(match batch {
            bounded_rolling_file::IncarnationReadBatch::Current { incarnation, batch } => {
                let incarnation: fedi_decentralized_service_fleet_manager::SafeEventJournalIncarnation = incarnation
                    .as_str()
                    .parse()
                    .expect("storage returns a validated UUIDv7");
                FetchSafeEventJournalResponse::Current {
                    incarnation: incarnation.clone(),
                    jsonl: batch.records,
                    next_cursor: batch.next_cursor.map(|cursor| SafeEventCursor {
                        incarnation,
                        segment: cursor.segment,
                        offset: cursor.offset,
                    }),
                    continuity_gap: batch.continuity_gap,
                }
            }
            bounded_rolling_file::IncarnationReadBatch::IncarnationChanged { incarnation } => {
                FetchSafeEventJournalResponse::IncarnationChanged {
                    incarnation: incarnation
                        .as_str()
                        .parse()
                        .expect("storage returns a validated UUIDv7"),
                }
            }
        })
    }
}

fn map_journal_read_error() -> ServiceError {
    ServiceError::with_code(
        ServiceErrorCode::Unavailable,
        "safe-event journal temporarily unavailable",
    )
}

fn open_journal_infos(
    directories: Vec<(
        fedi_decentralized_service_fleet_manager::SafeEventJournal,
        std::path::PathBuf,
    )>,
) -> std::io::Result<Vec<SafeEventJournalInfo>> {
    directories
        .into_iter()
        .map(|(journal, directory)| {
            let incarnation = bounded_rolling_file::open_incarnation(directory)?;
            Ok(SafeEventJournalInfo {
                journal,
                incarnation: incarnation
                    .as_str()
                    .parse()
                    .expect("storage returns a validated UUIDv7"),
            })
        })
        .collect()
}

fn fetch_journal_directory(
    directory: std::path::PathBuf,
    request: FetchSafeEventJournalRequest,
) -> std::io::Result<bounded_rolling_file::IncarnationReadBatch> {
    if request
        .cursor
        .as_ref()
        .is_some_and(|cursor| cursor.incarnation != request.incarnation)
    {
        return bounded_rolling_file::open_incarnation(directory).map(|incarnation| {
            bounded_rolling_file::IncarnationReadBatch::IncarnationChanged { incarnation }
        });
    }
    let cursor = request
        .cursor
        .map(|cursor| bounded_rolling_file::ReadCursor {
            segment: cursor.segment,
            offset: cursor.offset,
        });
    bounded_rolling_file::read_batch_for_incarnation(
        directory,
        request.incarnation.as_str(),
        cursor,
        MAX_SAFE_EVENT_BATCH_BYTES,
        MAX_SAFE_EVENT_RECORD_BYTES,
    )
}

fn authorize_telemetry(fleet: &Fleet, capability: &TelemetryCapability) -> TelemetryResult<()> {
    match fleet.authorize_telemetry(capability) {
        Ok(()) => Ok(()),
        Err(TelemetryAccessError::Unauthorized) => Err(ServiceError::with_code(
            ServiceErrorCode::PermissionDenied,
            "guardian telemetry access denied",
        )),
        Err(TelemetryAccessError::Unavailable) => Err(ServiceError::with_code(
            ServiceErrorCode::Unavailable,
            "guardian telemetry temporarily unavailable",
        )),
    }
}

fn header_text(value: Option<&reqwest::header::HeaderValue>) -> Option<String> {
    value
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned)
}

#[cfg(test)]
mod tests {
    use bounded_rolling_file::{Config as JournalConfig, RollingFileAppender};
    use fedi_decentralized_service_fleet_manager::{
        QuoteId, SafeEventJournal, SafeEventJournalIncarnation, SeatId,
    };
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    use tokio::net::TcpListener;

    use super::*;

    fn client() -> reqwest::Client {
        reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap()
    }

    async fn serve_once(response: impl Into<Vec<u8>>) -> u16 {
        let response = response.into();
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0_u8; 4096];
            let read = stream.read(&mut request).await.unwrap();
            assert!(String::from_utf8_lossy(&request[..read]).starts_with("GET /metrics HTTP/1.1"));
            stream.write_all(&response).await.unwrap();
            stream.shutdown().await.unwrap();
        });
        port
    }

    #[tokio::test]
    async fn rejects_untrusted_response_metadata_without_following_redirects() {
        let response = b"HTTP/1.1 302 Found\r\nContent-Type: application/openmetrics-text; version=1.0.0\r\nContent-Encoding: identity\r\nLocation: http://127.0.0.1:1/private\r\nContent-Length: 5\r\nConnection: close\r\n\r\n\x00a\xffb\n";
        let port = serve_once(response).await;

        let result = fetch_guardian_metrics(&client(), port, 5)
            .await
            .unwrap_err();
        assert_eq!(result.code(), ServiceErrorCode::Unavailable);
    }

    #[tokio::test]
    async fn emits_only_independently_valid_allowlisted_families() {
        let body = format!(
            "fm_app_start_ts{{version=\"legacy\",version_hash=\"legacy-build\"}} 1\n\
             fm_backup_counts{{timeframe=\"1d\"}} 7\n\
             fm_client_api_requests_total{{method=\"secret\",peer_id=\"0\",result=\"success\"}} 1\n\
             fm_jsonrpc_api_request_response_code_total{{method=\"secret\",code=\"0\",type=\"default\"}} 1\n\
             fm_consensus_session_count{{private=\"secret\"}} 8\n\
             fm_future_private_value{{token=\"secret\"}} 9\n"
        );
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/plain; version=0.0.4\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let port = serve_once(response).await;

        let result = fetch_guardian_metrics(&client(), port, 4096).await.unwrap();
        let text = String::from_utf8(result.body).unwrap();
        assert!(text.contains("fm_app_start_ts{"));
        assert!(text.contains("fm_backup_counts{timeframe=\"1d\"} 7"));
        assert!(!text.contains("secret"));
        assert!(!text.contains("client_api"));
        assert!(!text.contains("jsonrpc_api"));
        assert!(!text.contains("consensus_session_count"));
        assert!(!text.contains("future_private"));
        assert!(!text.contains("fman_id"));
    }

    #[tokio::test]
    async fn projects_canonical_api_metrics_from_any_release() {
        for release in [
            "fm_app_start_ts{version=\"legacy\",version_hash=\"old-build\"} 1\n",
            "fm_app_start_ts{version=\"future\",version_hash=\"new-build\"} 1\n",
        ] {
            let body = format!(
                "{release}fm_jsonrpc_api_request_response_code_total{{method=\"status\",code=\"0\",type=\"default\"}} 1\n"
            );
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/plain; version=0.0.4\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let port = serve_once(response).await;

            let result = fetch_guardian_metrics(&client(), port, 4096).await.unwrap();
            let text = String::from_utf8(result.body).unwrap();
            assert!(text.contains("fm_app_start_ts{"));
            assert!(text.contains(
                "fm_jsonrpc_api_request_response_code_total{code=\"0\",method=\"status\",type=\"default\"} 1"
            ));
        }
    }

    #[tokio::test]
    async fn projects_valid_metrics_without_a_release_marker() {
        let body = "fm_backup_counts{timeframe=\"1d\"} 7\n";
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/plain; version=0.0.4\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let port = serve_once(response.as_bytes()).await;

        let result = fetch_guardian_metrics(&client(), port, 4096).await.unwrap();
        assert_eq!(
            String::from_utf8(result.body).unwrap(),
            "fm_backup_counts{timeframe=\"1d\"} 7\n"
        );
    }

    #[tokio::test]
    async fn rejects_declared_and_streamed_responses_over_the_byte_limit() {
        let declared = serve_once(
            b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 6\r\nConnection: close\r\n\r\nabcdef",
        )
        .await;
        let error = fetch_guardian_metrics(&client(), declared, 5)
            .await
            .unwrap_err();
        assert_eq!(error.code(), ServiceErrorCode::Unavailable);

        let streamed = serve_once(
            b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n3\r\nabc\r\n3\r\ndef\r\n0\r\n\r\n",
        )
        .await;
        let error = fetch_guardian_metrics(&client(), streamed, 5)
            .await
            .unwrap_err();
        assert_eq!(error.code(), ServiceErrorCode::Unavailable);
    }

    fn incarnation(directory: &std::path::Path) -> SafeEventJournalIncarnation {
        bounded_rolling_file::open_incarnation(directory)
            .unwrap()
            .as_str()
            .parse()
            .unwrap()
    }

    fn journal_request(
        incarnation: SafeEventJournalIncarnation,
        cursor: Option<SafeEventCursor>,
    ) -> FetchSafeEventJournalRequest {
        FetchSafeEventJournalRequest {
            capability: TelemetryCapability::from_bytes([3; 32]),
            journal: SafeEventJournal::Fman,
            incarnation,
            cursor,
        }
    }

    #[test]
    fn journal_adapter_binds_current_stale_mixed_and_cross_journal_cursors() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let first_incarnation = incarnation(first.path());
        let second_incarnation = incarnation(second.path());
        let mut writer = RollingFileAppender::open(
            first.path(),
            JournalConfig {
                max_file_bytes: 1024,
                max_files: 2,
            },
        )
        .unwrap();
        writer.append_record(b"{\"safe\":true}\n").unwrap();
        drop(writer);

        let current = fetch_journal_directory(
            first.path().to_owned(),
            journal_request(first_incarnation.clone(), None),
        )
        .unwrap();
        let bounded_rolling_file::IncarnationReadBatch::Current { batch, .. } = current else {
            panic!("listed incarnation must be current");
        };
        assert_eq!(batch.records, b"{\"safe\":true}\n");
        let coordinates = batch.next_cursor.unwrap();
        let cursor = SafeEventCursor {
            incarnation: first_incarnation.clone(),
            segment: coordinates.segment,
            offset: coordinates.offset,
        };

        let stale = fetch_journal_directory(
            second.path().to_owned(),
            journal_request(first_incarnation.clone(), Some(cursor.clone())),
        )
        .unwrap();
        assert!(matches!(
            stale,
            bounded_rolling_file::IncarnationReadBatch::IncarnationChanged { .. }
        ));

        let segment = first.path().join("events-0.jsonl");
        std::fs::remove_file(&segment).unwrap();
        std::os::unix::fs::symlink(first.path().join("missing"), &segment).unwrap();
        let mixed = fetch_journal_directory(
            first.path().to_owned(),
            journal_request(second_incarnation, Some(cursor)),
        )
        .unwrap();
        assert!(matches!(
            mixed,
            bounded_rolling_file::IncarnationReadBatch::IncarnationChanged { .. }
        ));

        std::fs::remove_dir_all(first.path()).unwrap();
        std::fs::create_dir(first.path()).unwrap();
        let recreated = fetch_journal_directory(
            first.path().to_owned(),
            journal_request(first_incarnation, None),
        )
        .unwrap();
        assert!(matches!(
            recreated,
            bounded_rolling_file::IncarnationReadBatch::IncarnationChanged { .. }
        ));

        let error = map_journal_read_error();
        assert_eq!(error.code(), ServiceErrorCode::Unavailable);
        assert!(!format!("{error:?}").contains("missing"));
    }

    #[test]
    fn listing_initializes_distinct_fman_current_and_retained_seat_journals() {
        let root = tempfile::tempdir().unwrap();
        let selectors = [
            SafeEventJournal::Fman,
            SafeEventJournal::Seat {
                seat_id: SeatId::from(QuoteId([1; 32])),
            },
            SafeEventJournal::Seat {
                seat_id: SeatId::from(QuoteId([2; 32])),
            },
        ];
        let directories = selectors
            .iter()
            .cloned()
            .enumerate()
            .map(|(index, journal)| (journal, root.path().join(index.to_string())))
            .collect();
        let first = open_journal_infos(directories).unwrap();
        let second = open_journal_infos(
            first
                .iter()
                .enumerate()
                .map(|(index, info)| (info.journal.clone(), root.path().join(index.to_string())))
                .collect(),
        )
        .unwrap();

        assert_eq!(first, second);
        assert_eq!(
            first
                .iter()
                .map(|info| info.incarnation.as_str())
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            3
        );
    }
}
