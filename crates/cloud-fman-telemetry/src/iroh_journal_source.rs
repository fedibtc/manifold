//! Bounded authenticated Iroh sessions for typed safe-journal RPCs.

use std::{str::FromStr as _, time::Duration};

use async_trait::async_trait;
use fedi_decentralized_service_fleet_manager::{
    FetchSafeEventJournalRequest, FetchSafeEventJournalResponse, GUARDIAN_TELEMETRY_ALPN,
    GuardianTelemetryApi as _, GuardianTelemetryApiClient, ListSafeEventJournalsRequest,
    ListSafeEventJournalsResponse, MAX_SAFE_EVENT_BATCH_BYTES, TelemetryCapability,
};
use fedi_iroh_rpc::{
    RpcClient,
    iroh::{Endpoint, EndpointAddr, EndpointId},
};

use crate::{journal_poller::PollError, journal_target::WorkTarget, store::JournalStreamState};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const RPC_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_RPC_REQUEST_BYTES: usize = 8 * 1024;
const MAX_RPC_OVERHEAD_BYTES: usize = 64 * 1024;

#[async_trait]
pub(crate) trait JournalSource: Send + Sync {
    async fn connect(&self, target: &WorkTarget) -> Result<Box<dyn JournalSession>, PollError>;
}

#[async_trait]
pub(crate) trait JournalSession: Send {
    async fn list(&mut self) -> Result<ListSafeEventJournalsResponse, PollError>;
    async fn fetch(
        &mut self,
        stream: &JournalStreamState,
    ) -> Result<FetchSafeEventJournalResponse, PollError>;
}

pub(crate) struct IrohJournalSource {
    endpoint: Endpoint,
    address_override: Option<EndpointAddr>,
}

impl IrohJournalSource {
    pub(crate) fn with_optional_address(
        endpoint: Endpoint,
        address_override: Option<EndpointAddr>,
    ) -> Self {
        Self {
            endpoint,
            address_override,
        }
    }

    #[cfg(test)]
    fn with_address(endpoint: Endpoint, address: EndpointAddr) -> Self {
        Self {
            endpoint,
            address_override: Some(address),
        }
    }
}

struct IrohJournalSession {
    client: GuardianTelemetryApiClient,
    capability: TelemetryCapability,
}

#[async_trait]
impl JournalSource for IrohJournalSource {
    async fn connect(&self, target: &WorkTarget) -> Result<Box<dyn JournalSession>, PollError> {
        let endpoint_id =
            EndpointId::from_str(target.endpoint_id()).map_err(|_| PollError::Transient)?;
        let address = self
            .address_override
            .clone()
            .filter(|address| address.id == endpoint_id)
            .unwrap_or_else(|| EndpointAddr::new(endpoint_id));
        let connection = tokio::time::timeout(
            CONNECT_TIMEOUT,
            self.endpoint.connect(address, GUARDIAN_TELEMETRY_ALPN),
        )
        .await
        .map_err(|_| PollError::Transient)?
        .map_err(|_| PollError::Transient)?;
        Ok(Box::new(IrohJournalSession {
            client: GuardianTelemetryApiClient::from_rpc_client(RpcClient::with_limits(
                connection,
                MAX_RPC_REQUEST_BYTES,
                MAX_SAFE_EVENT_BATCH_BYTES + MAX_RPC_OVERHEAD_BYTES,
            )),
            capability: target.capability().clone(),
        }))
    }
}

#[async_trait]
impl JournalSession for IrohJournalSession {
    async fn list(&mut self) -> Result<ListSafeEventJournalsResponse, PollError> {
        tokio::time::timeout(
            RPC_TIMEOUT,
            self.client
                .list_safe_event_journals(ListSafeEventJournalsRequest {
                    capability: self.capability.clone(),
                }),
        )
        .await
        .map_err(|_| PollError::Transient)?
        .map_err(|_| PollError::Transient)
    }

    async fn fetch(
        &mut self,
        stream: &JournalStreamState,
    ) -> Result<FetchSafeEventJournalResponse, PollError> {
        tokio::time::timeout(
            RPC_TIMEOUT,
            self.client
                .fetch_safe_event_journal(FetchSafeEventJournalRequest {
                    capability: self.capability.clone(),
                    journal: stream.journal.clone(),
                    incarnation: stream.incarnation.clone(),
                    cursor: stream.cursor.clone(),
                }),
        )
        .await
        .map_err(|_| PollError::Transient)?
        .map_err(|_| PollError::Transient)
    }
}

#[cfg(test)]
#[path = "iroh_journal_source_tests.rs"]
mod tests;
