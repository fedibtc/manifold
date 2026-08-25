//! Production Iroh safe-journal wire tests.

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

use fedi_decentralized_service_fleet_manager::{
    FetchSafeEventJournalRequest, FetchSafeEventJournalResponse, GuardianMetricsResponse,
    GuardianTelemetryApi, GuardianTelemetryApiServer, ListGuardianTelemetrySeatsRequest,
    ListGuardianTelemetrySeatsResponse, ListSafeEventJournalsRequest,
    ListSafeEventJournalsResponse, SafeEventCursor, SafeEventJournal, SafeEventJournalIncarnation,
    SafeEventJournalInfo, ScrapeGuardianMetricsRequest, TelemetryCapability, TelemetryResult,
};
use fedi_iroh_rpc::{
    IrohProtocol,
    iroh::{Endpoint, RelayMode, endpoint::presets, protocol::Router},
};

use super::*;
use crate::{
    journal_target::WorkTarget, journal_types::JournalStreamId, store::JournalStreamState,
};

#[derive(Clone)]
struct TestService {
    incarnation: SafeEventJournalIncarnation,
    requests: Arc<Mutex<Vec<FetchSafeEventJournalRequest>>>,
    oversized: Arc<AtomicBool>,
}

impl GuardianTelemetryApi for TestService {
    async fn list_guardian_telemetry_seats(
        &self,
        _: ListGuardianTelemetrySeatsRequest,
    ) -> TelemetryResult<ListGuardianTelemetrySeatsResponse> {
        Ok(ListGuardianTelemetrySeatsResponse { seats: Vec::new() })
    }

    async fn scrape_guardian_metrics(
        &self,
        _: ScrapeGuardianMetricsRequest,
    ) -> TelemetryResult<GuardianMetricsResponse> {
        Ok(GuardianMetricsResponse {
            status_code: 200,
            content_type: None,
            content_encoding: None,
            body: Vec::new(),
        })
    }

    async fn list_safe_event_journals(
        &self,
        request: ListSafeEventJournalsRequest,
    ) -> TelemetryResult<ListSafeEventJournalsResponse> {
        assert_eq!(request.capability.as_bytes(), &[7; 32]);
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
        assert_eq!(request.capability.as_bytes(), &[7; 32]);
        self.requests.lock().unwrap().push(request.clone());
        Ok(FetchSafeEventJournalResponse::Current {
            incarnation: self.incarnation.clone(),
            jsonl: if self.oversized.load(Ordering::SeqCst) {
                vec![b'x'; 2 * 1024 * 1024]
            } else {
                b"{\"fields\":{\"safe_to_share\":true}}\n".to_vec()
            },
            next_cursor: Some(SafeEventCursor {
                incarnation: self.incarnation.clone(),
                segment: 4,
                offset: 9,
            }),
            continuity_gap: false,
        })
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn production_alpn_client_sends_expected_incarnation_cursor_and_capability() {
    let incarnation: SafeEventJournalIncarnation =
        "018f22d0-4e5f-7abc-8def-0123456789ab".parse().unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let oversized = Arc::new(AtomicBool::new(false));
    let server_endpoint = Endpoint::builder(presets::N0)
        .relay_mode(RelayMode::Disabled)
        .bind()
        .await
        .unwrap();
    let router = Router::builder(server_endpoint)
        .accept(
            GUARDIAN_TELEMETRY_ALPN,
            IrohProtocol::new(GuardianTelemetryApiServer::new(TestService {
                incarnation: incarnation.clone(),
                requests: requests.clone(),
                oversized: oversized.clone(),
            })),
        )
        .spawn();
    let client_endpoint = Endpoint::builder(presets::N0)
        .relay_mode(RelayMode::Disabled)
        .bind()
        .await
        .unwrap();
    let source = IrohJournalSource::with_address(client_endpoint, router.endpoint().addr());
    let target = WorkTarget::new(
        "aa".repeat(16),
        1,
        router.endpoint().id().to_string(),
        TelemetryCapability::from_bytes([7; 32]),
        "11".repeat(32),
        "calm-tern".into(),
    );
    let cursor = SafeEventCursor {
        incarnation: incarnation.clone(),
        segment: 3,
        offset: 8,
    };
    let state = JournalStreamState {
        stream_id: JournalStreamId::parse("bb".repeat(16)).unwrap(),
        journal: SafeEventJournal::Fman,
        incarnation: incarnation.clone(),
        cursor: Some(cursor.clone()),
        observed_generation: 0,
    };

    let mut session = source.connect(&target).await.unwrap();
    assert_eq!(session.list().await.unwrap().journals.len(), 1);
    assert!(matches!(
        session.fetch(&state).await.unwrap(),
        FetchSafeEventJournalResponse::Current { .. }
    ));
    let request = requests.lock().unwrap().pop().unwrap();
    assert_eq!(request.incarnation, incarnation);
    assert_eq!(request.cursor, Some(cursor));
    assert_eq!(request.journal, SafeEventJournal::Fman);
    oversized.store(true, Ordering::SeqCst);
    assert!(matches!(
        session.fetch(&state).await,
        Err(PollError::Transient)
    ));
    router.shutdown().await.unwrap();
}
