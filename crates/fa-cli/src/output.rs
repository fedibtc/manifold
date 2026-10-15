use std::io::{self, Write};

use fa_client::{CreationStage, FaState};
use fedi_decentralized_service_manifold_guardian::{FaId, FedimintdVersion};
use serde::Serialize;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ApiEndpointJson<'a> {
    transport: &'a str,
    url: &'a str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DiscoveryCandidateJson<'a> {
    #[serde(rename = "fmanPubkey")]
    mg_pubkey: String,
    advertised_price_msats: u64,
    federation_sizes: &'a [u16],
    fedimintd_version: &'a FedimintdVersion,
    claimed_issuer: String,
    api_endpoints: Vec<ApiEndpointJson<'a>>,
    locator: &'a fa_client::Locator,
    issued_at: u64,
    expires_at: u64,
}

impl<'a> From<&'a fa_client::EligibleMgCandidate> for DiscoveryCandidateJson<'a> {
    fn from(candidate: &'a fa_client::EligibleMgCandidate) -> Self {
        Self {
            mg_pubkey: candidate.mg_id().to_string(),
            advertised_price_msats: candidate.advertised_price_msats(),
            federation_sizes: &candidate.availability().federation_sizes,
            fedimintd_version: &candidate.availability().fedimintd_version,
            claimed_issuer: candidate.claimed_issuer().to_string(),
            api_endpoints: candidate
                .api_endpoints()
                .iter()
                .map(|endpoint| ApiEndpointJson {
                    transport: &endpoint.transport,
                    url: &endpoint.url,
                })
                .collect(),
            locator: candidate.locator(),
            issued_at: candidate.issued_at().0,
            expires_at: candidate.expires_at().0,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SelectionSeatJson<'a> {
    #[serde(rename = "fmanPubkey")]
    mg_pubkey: String,
    advertised_price_msats: u64,
    locator: &'a fa_client::Locator,
    issuer: String,
    holder: String,
    trust_level: u64,
    provenance: &'static str,
}

impl<'a> From<&'a fa_client::SelectedMgSeat> for SelectionSeatJson<'a> {
    fn from(seat: &'a fa_client::SelectedMgSeat) -> Self {
        Self {
            mg_pubkey: seat.candidate().mg_id().to_string(),
            advertised_price_msats: seat.advertised_price_msats(),
            locator: seat.candidate().locator(),
            issuer: seat.candidate().badge().issuer().to_string(),
            holder: seat.candidate().badge().holder().to_string(),
            trust_level: seat.candidate().badge().badge().trust_level,
            provenance: seat.provenance().code(),
        }
    }
}

#[derive(Serialize)]
struct RejectionJson {
    author: String,
    reason: &'static str,
}

#[derive(Serialize)]
struct DiscoveryJson<'a> {
    seen: usize,
    eligible: usize,
    candidates: Vec<DiscoveryCandidateJson<'a>>,
    rejected: Vec<RejectionJson>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SelectionPreviewJson<'a> {
    seen: usize,
    eligible: usize,
    selected: usize,
    fedimintd_dkg_version: String,
    total_advertised_msats: u64,
    seats: Vec<SelectionSeatJson<'a>>,
    rejected: Vec<RejectionJson>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LiquidityProviderJson<'a> {
    provider_pubkey: &'a str,
    endpoint: &'a str,
    advertisement_hash: String,
    advertisement: &'a fedi_decentralized_service_liquidity_manager::LiquidityProviderAdvertisement,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LiquidityRejectionJson<'a> {
    provider_pubkey: Option<&'a str>,
    reason: &'static str,
}

#[derive(Serialize)]
struct LiquidityDiscoveryJson<'a> {
    providers: Vec<LiquidityProviderJson<'a>>,
    rejected: Vec<LiquidityRejectionJson<'a>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MetadataConsensusJson<'a> {
    field: &'a str,
    value: &'a str,
    consensus_reached: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DecommissionJson<'a> {
    decommissioned: &'a [u16],
    already_decommissioned: &'a [u16],
    refused: Vec<RefusedSeatJson<'a>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RefusedSeatJson<'a> {
    index: u16,
    reason: &'a str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GuardianFeeConsensusJson {
    send_ppm: u32,
    consensus_reached: bool,
}

/// Selects stable JSON or changeable human-readable output.
#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) enum OutputFormat {
    Human,
    Json,
}

impl OutputFormat {
    /// Converts the CLI's `--json` flag at the argument boundary.
    pub(crate) fn from_json_flag(json: bool) -> Self {
        if json { Self::Json } else { Self::Human }
    }
}

/// Writes the CLI's stdout and stderr contracts to independently testable streams.
pub(crate) struct CliOutput<'a> {
    stdout: Box<dyn Write + 'a>,
    stderr: Box<dyn Write + 'a>,
}

impl CliOutput<'static> {
    /// Connects output to the process stdout and stderr streams.
    pub(crate) fn stdio() -> Self {
        Self::new(io::stdout(), io::stderr())
    }
}

impl<'a> CliOutput<'a> {
    /// Connects output to the supplied stdout and stderr streams.
    fn new(stdout: impl Write + 'a, stderr: impl Write + 'a) -> Self {
        Self {
            stdout: Box::new(stdout),
            stderr: Box::new(stderr),
        }
    }

    /// Writes the stable JSON initialization result to stdout.
    pub(crate) fn init(&mut self, fa_id: FaId, state: &FaState) -> anyhow::Result<()> {
        serde_json::to_writer(
            &mut self.stdout,
            &serde_json::json!({
                "fiPubkey": hex::encode(fa_id.0.serialize()),
                "state": state,
            }),
        )?;
        writeln!(self.stdout)?;
        Ok(())
    }

    /// Writes a status snapshot in the selected output format.
    pub(crate) fn snapshot(
        &mut self,
        snapshot: &FaState,
        format: OutputFormat,
    ) -> anyhow::Result<()> {
        if format == OutputFormat::Json {
            serde_json::to_writer(&mut self.stdout, snapshot)?;
            writeln!(self.stdout)?;
        } else {
            self.status_text(snapshot)?;
        }
        Ok(())
    }

    fn status_text(&mut self, state: &FaState) -> anyhow::Result<()> {
        let last_error = match state {
            FaState::Idle => {
                writeln!(self.stdout, "FA state: idle")?;
                None
            }
            FaState::Restoring(recovery) => {
                writeln!(self.stdout, "FA state: checking backup")?;
                recovery.last_error
            }
            FaState::Creating(creation) => {
                let accepted = creation
                    .slots
                    .iter()
                    .filter(|slot| slot.seat_id.is_some())
                    .count();
                writeln!(
                    self.stdout,
                    "seats: {accepted} of {}",
                    creation.federation_size.0
                )?;
                match &creation.stage {
                    CreationStage::NeedsSeats {
                        count,
                        reauthorization,
                    } => {
                        writeln!(self.stdout, "creation state: needs {count} seats")?;
                        if let Some(reason) = reauthorization {
                            writeln!(self.stdout, "purchase ended: {reason:?}")?;
                        }
                    }
                    CreationStage::Buying => {
                        writeln!(self.stdout, "creation state: buying seats")?;
                    }
                    CreationStage::Forming { phase, .. } => {
                        writeln!(self.stdout, "formation state: {phase:?}")?;
                    }
                }
                creation.last_error
            }
            FaState::Active(federation) => {
                writeln!(self.stdout, "{}", federation.federation_invite.0)?;
                let freshness = match federation.freshness {
                    fa_client::FormationFreshness::Fresh => "fresh",
                    fa_client::FormationFreshness::Unsynced => "unsynced",
                };
                writeln!(self.stdout, "federation state: {freshness}")?;
                federation.last_error
            }
        };
        if let Some(error) = last_error {
            writeln!(self.stdout, "last error: {error:?}")?;
        }
        Ok(())
    }

    /// Writes the human wallet-funding notice and suppresses it in JSON mode.
    pub(crate) fn wallet_funded(
        &mut self,
        amount: fedimint_core::Amount,
        format: OutputFormat,
    ) -> anyhow::Result<()> {
        if format == OutputFormat::Human {
            writeln!(self.stderr, "payment wallet funded with {amount}")?;
        }
        Ok(())
    }

    /// Writes the result of initializing or reopening the payment client.
    pub(crate) fn payment_wallet_joined(
        &mut self,
        federation_id: fedimint_core::config::FederationId,
        balance: fedimint_core::Amount,
        format: OutputFormat,
    ) -> anyhow::Result<()> {
        if format == OutputFormat::Json {
            serde_json::to_writer(
                &mut self.stdout,
                &serde_json::json!({
                    "federationId": federation_id.to_string(),
                    "balanceMsats": balance.msats,
                }),
            )?;
            writeln!(self.stdout)?;
        } else {
            writeln!(
                self.stdout,
                "joined payment federation {federation_id} (balance {} msat)",
                balance.msats,
            )?;
        }
        Ok(())
    }

    /// Writes the current spendable payment-wallet balance.
    pub(crate) fn payment_wallet_balance(
        &mut self,
        federation_id: fedimint_core::config::FederationId,
        balance: fedimint_core::Amount,
        format: OutputFormat,
    ) -> anyhow::Result<()> {
        if format == OutputFormat::Json {
            serde_json::to_writer(
                &mut self.stdout,
                &serde_json::json!({
                    "federationId": federation_id.to_string(),
                    "balanceMsats": balance.msats,
                }),
            )?;
            writeln!(self.stdout)?;
        } else {
            writeln!(self.stdout, "{} msat", balance.msats)?;
        }
        Ok(())
    }

    /// Writes independently observed accepted-transaction accounting for the
    /// reopened FA payment wallet.
    pub(crate) fn payment_wallet_accounting(
        &mut self,
        federation_id: fedimint_core::config::FederationId,
        balance: fedimint_core::Amount,
        accounting: crate::payer::PaymentWalletAccounting,
        format: OutputFormat,
    ) -> anyhow::Result<()> {
        if format == OutputFormat::Json {
            serde_json::to_writer(
                &mut self.stdout,
                &serde_json::json!({
                    "federationId": federation_id.to_string(),
                    "balanceMsats": balance.msats,
                    "receivedInputMsats": accounting.received_input_msats,
                    "receiveFeeMsats": accounting.receive_fee_msats,
                    "setupOutputMsats": accounting.setup_output_msats,
                    "setupFeeMsats": accounting.setup_fee_msats,
                    "setupTransactionCount": accounting.setup_transaction_count,
                }),
            )?;
            writeln!(self.stdout)?;
        } else {
            writeln!(
                self.stdout,
                "balance={} received={} receive-fees={} setup-outputs={} setup-fees={} setup-transactions={}",
                balance.msats,
                accounting.received_input_msats,
                accounting.receive_fee_msats,
                accounting.setup_output_msats,
                accounting.setup_fee_msats,
                accounting.setup_transaction_count,
            )?;
        }
        Ok(())
    }

    /// Writes an on-chain funding address.
    pub(crate) fn payment_wallet_deposit_address(
        &mut self,
        federation_id: fedimint_core::config::FederationId,
        address: &str,
        format: OutputFormat,
    ) -> anyhow::Result<()> {
        if format == OutputFormat::Json {
            serde_json::to_writer(
                &mut self.stdout,
                &serde_json::json!({
                    "federationId": federation_id.to_string(),
                    "address": address,
                }),
            )?;
            writeln!(self.stdout)?;
        } else {
            writeln!(self.stdout, "{address}")?;
        }
        Ok(())
    }

    /// Writes the balance that satisfied a direct-funding wait.
    pub(crate) fn payment_wallet_balance_reached(
        &mut self,
        federation_id: fedimint_core::config::FederationId,
        balance: fedimint_core::Amount,
        minimum: fedimint_core::Amount,
        format: OutputFormat,
    ) -> anyhow::Result<()> {
        if format == OutputFormat::Json {
            serde_json::to_writer(
                &mut self.stdout,
                &serde_json::json!({
                    "federationId": federation_id.to_string(),
                    "balanceMsats": balance.msats,
                    "minimumMsats": minimum.msats,
                }),
            )?;
            writeln!(self.stdout)?;
        } else {
            writeln!(
                self.stdout,
                "payment wallet funded: {} msat (minimum {} msat)",
                balance.msats, minimum.msats,
            )?;
        }
        Ok(())
    }

    /// Writes a newly created Lightning funding invoice and its recovery key.
    pub(crate) fn payment_wallet_invoice(
        &mut self,
        federation_id: fedimint_core::config::FederationId,
        invoice: &str,
        operation_id: fedimint_core::core::OperationId,
        amount: fedimint_core::Amount,
        format: OutputFormat,
    ) -> anyhow::Result<()> {
        if format == OutputFormat::Json {
            serde_json::to_writer(
                &mut self.stdout,
                &serde_json::json!({
                    "federationId": federation_id.to_string(),
                    "invoice": invoice,
                    "operationId": operation_id.fmt_full().to_string(),
                    "amountMsats": amount.msats,
                }),
            )?;
            writeln!(self.stdout)?;
        } else {
            writeln!(self.stdout, "invoice: {invoice}")?;
            writeln!(self.stdout, "operation id: {}", operation_id.fmt_full())?;
        }
        Ok(())
    }

    /// Writes the terminal projection of a Lightning receive operation.
    pub(crate) fn payment_wallet_invoice_settled(
        &mut self,
        federation_id: fedimint_core::config::FederationId,
        operation_id: fedimint_core::core::OperationId,
        state: crate::wallet::PaymentWalletInvoiceState,
        balance: fedimint_core::Amount,
        format: OutputFormat,
    ) -> anyhow::Result<()> {
        let state = match state {
            crate::wallet::PaymentWalletInvoiceState::Claimed => "claimed",
            crate::wallet::PaymentWalletInvoiceState::Expired => "expired",
            crate::wallet::PaymentWalletInvoiceState::Failure => "failure",
        };
        if format == OutputFormat::Json {
            serde_json::to_writer(
                &mut self.stdout,
                &serde_json::json!({
                    "federationId": federation_id.to_string(),
                    "operationId": operation_id.fmt_full().to_string(),
                    "state": state,
                    "balanceMsats": balance.msats,
                }),
            )?;
            writeln!(self.stdout)?;
        } else {
            writeln!(
                self.stdout,
                "invoice operation {}: {state}; balance {} msat",
                operation_id.fmt_full(),
                balance.msats,
            )?;
        }
        Ok(())
    }

    /// Writes the accepted stability-pool guardian-fee deposit.
    pub(crate) fn payment_wallet_guardian_fee_remitted(
        &mut self,
        federation_id: fedimint_core::config::FederationId,
        operation_id: fedimint_core::core::OperationId,
        amount: fedimint_core::Amount,
        format: OutputFormat,
    ) -> anyhow::Result<()> {
        if format == OutputFormat::Json {
            serde_json::to_writer(
                &mut self.stdout,
                &serde_json::json!({
                    "federationId": federation_id.to_string(),
                    "operationId": operation_id.fmt_full().to_string(),
                    "amountMsats": amount.msats,
                }),
            )?;
            writeln!(self.stdout)?;
        } else {
            writeln!(
                self.stdout,
                "remitted {} msat to guardian-fee account (operation {})",
                amount.msats,
                operation_id.fmt_full(),
            )?;
        }
        Ok(())
    }

    /// Writes one discovery run's candidates and typed rejections.
    pub(crate) fn discovery(
        &mut self,
        discovery: &fa_client::MgDiscovery,
        format: OutputFormat,
    ) -> anyhow::Result<()> {
        if format == OutputFormat::Json {
            let candidates = discovery
                .candidates
                .iter()
                .map(DiscoveryCandidateJson::from)
                .collect::<Vec<_>>();
            serde_json::to_writer(
                &mut self.stdout,
                &DiscoveryJson {
                    seen: discovery.seen(),
                    eligible: discovery.candidates.len(),
                    candidates,
                    rejected: rejections_json(&discovery.rejected),
                },
            )?;
            writeln!(self.stdout)?;
            return Ok(());
        }
        writeln!(
            self.stdout,
            "discovered {} advertisement(s): {} eligible, {} rejected",
            discovery.seen(),
            discovery.candidates.len(),
            discovery.rejected.len(),
        )?;
        for candidate in &discovery.candidates {
            writeln!(
                self.stdout,
                "  {}: price={} msat sizes={:?} issuer(claimed)={}",
                candidate.mg_id(),
                candidate.advertised_price_msats(),
                candidate.availability().federation_sizes,
                candidate.claimed_issuer(),
            )?;
        }
        self.rejections_human(&discovery.rejected)?;
        Ok(())
    }

    /// Writes one read-only selection preview.
    pub(crate) fn selection_preview(
        &mut self,
        preview: &fa_client::MgSelectionPreview,
        format: OutputFormat,
    ) -> anyhow::Result<()> {
        if format == OutputFormat::Json {
            let seats = preview
                .seats()
                .iter()
                .map(SelectionSeatJson::from)
                .collect::<Vec<_>>();
            serde_json::to_writer(
                &mut self.stdout,
                &SelectionPreviewJson {
                    seen: preview.seen(),
                    eligible: preview.eligible(),
                    selected: preview.selected(),
                    fedimintd_dkg_version: preview.fedimintd_dkg_version().to_string(),
                    total_advertised_msats: preview.total_advertised_msats(),
                    seats,
                    rejected: rejections_json(preview.rejected()),
                },
            )?;
            writeln!(self.stdout)?;
            return Ok(());
        }
        writeln!(
            self.stdout,
            "selected {} seat(s) for fedimintd {} ({} seen, {} eligible), estimated total {} msat",
            preview.selected(),
            preview.fedimintd_dkg_version(),
            preview.seen(),
            preview.eligible(),
            preview.total_advertised_msats(),
        )?;
        for (position, seat) in preview.seats().iter().enumerate() {
            writeln!(
                self.stdout,
                "  seat {}: {} price={} msat issuer={} trust_level={} [{:?}]",
                position + 1,
                seat.candidate().mg_id(),
                seat.advertised_price_msats(),
                seat.candidate().badge().issuer(),
                seat.candidate().badge().badge().trust_level,
                seat.provenance(),
            )?;
        }
        self.rejections_human(preview.rejected())?;
        Ok(())
    }

    /// Writes one metadata mutation after fa-client's exact consensus readback.
    pub(crate) fn metadata_consensus(
        &mut self,
        field: &str,
        value: &str,
        format: OutputFormat,
    ) -> anyhow::Result<()> {
        if format == OutputFormat::Json {
            serde_json::to_writer(
                &mut self.stdout,
                &MetadataConsensusJson {
                    field,
                    value,
                    consensus_reached: true,
                },
            )?;
            writeln!(self.stdout)?;
        } else {
            writeln!(
                self.stdout,
                "consensus metadata {field} reached threshold consensus: {value}"
            )?;
        }
        Ok(())
    }

    /// Writes the per-seat result of a testing-only seat decommission.
    pub(crate) fn decommission(
        &mut self,
        outcome: &fa_client::DecommissionOutcome,
        format: OutputFormat,
    ) -> anyhow::Result<()> {
        if format == OutputFormat::Json {
            serde_json::to_writer(
                &mut self.stdout,
                &DecommissionJson {
                    decommissioned: &outcome.decommissioned,
                    already_decommissioned: &outcome.already_decommissioned,
                    refused: outcome
                        .refused
                        .iter()
                        .map(|(index, reason)| RefusedSeatJson {
                            index: *index,
                            reason,
                        })
                        .collect(),
                },
            )?;
            writeln!(self.stdout)?;
        } else {
            writeln!(
                self.stdout,
                "decommissioned {} seat(s), {} already decommissioned",
                outcome.decommissioned.len(),
                outcome.already_decommissioned.len()
            )?;
            for (index, reason) in &outcome.refused {
                writeln!(self.stdout, "seat {index} NOT decommissioned: {reason}")?;
            }
        }
        Ok(())
    }

    /// Writes guardian-fee success after fa-client verifies the consensus rate.
    pub(crate) fn guardian_fee_consensus(
        &mut self,
        send_ppm: u32,
        format: OutputFormat,
    ) -> anyhow::Result<()> {
        if format == OutputFormat::Json {
            serde_json::to_writer(
                &mut self.stdout,
                &GuardianFeeConsensusJson {
                    send_ppm,
                    consensus_reached: true,
                },
            )?;
            writeln!(self.stdout)?;
        } else {
            writeln!(
                self.stdout,
                "guardian fee {send_ppm} ppm reached threshold consensus"
            )?;
        }
        Ok(())
    }

    /// Writes one fully verified FLIP provider discovery run.
    pub(crate) fn liquidity_discovery(
        &mut self,
        discovery: &fa_client::LiquidityDiscovery,
        format: OutputFormat,
    ) -> anyhow::Result<()> {
        if format == OutputFormat::Json {
            let providers = discovery
                .providers
                .iter()
                .map(|provider| LiquidityProviderJson {
                    provider_pubkey: &provider.provider_pubkey().0,
                    endpoint: &provider.endpoint_url().0,
                    advertisement_hash: hex::encode(provider.advertisement_hash().0),
                    advertisement: provider.advertisement(),
                })
                .collect();
            let rejected = discovery
                .rejected
                .iter()
                .map(|(provider, reason)| LiquidityRejectionJson {
                    provider_pubkey: provider.as_ref().map(|provider| provider.0.as_str()),
                    reason: reason.code(),
                })
                .collect();
            serde_json::to_writer(
                &mut self.stdout,
                &LiquidityDiscoveryJson {
                    providers,
                    rejected,
                },
            )?;
            writeln!(self.stdout)?;
            return Ok(());
        }
        writeln!(
            self.stdout,
            "discovered {} admitted liquidity provider(s), {} rejected",
            discovery.providers.len(),
            discovery.rejected.len(),
        )?;
        for provider in &discovery.providers {
            writeln!(
                self.stdout,
                "  {}: {}",
                provider.provider_pubkey().0,
                provider.endpoint_url().0,
            )?;
        }
        for (provider, reason) in &discovery.rejected {
            writeln!(
                self.stdout,
                "  rejected {}: {}",
                provider
                    .as_ref()
                    .map_or("<unknown>", |provider| provider.0.as_str()),
                reason.code(),
            )?;
        }
        Ok(())
    }

    /// Writes one durable liquidity operation projection.
    pub(crate) fn liquidity_snapshot(
        &mut self,
        snapshot: &fa_client::LiquidityOperationSnapshot,
        format: OutputFormat,
    ) -> anyhow::Result<()> {
        if format == OutputFormat::Json {
            serde_json::to_writer(&mut self.stdout, snapshot)?;
            writeln!(self.stdout)?;
        } else {
            writeln!(
                self.stdout,
                "liquidity operation {}: {:?} via {}",
                snapshot.operation_id.0, snapshot.phase, snapshot.provider_pubkey.0,
            )?;
        }
        Ok(())
    }

    /// Writes one bounded durable liquidity-operation page.
    pub(crate) fn liquidity_page(
        &mut self,
        page: &fa_client::LiquidityOperationPage,
        format: OutputFormat,
    ) -> anyhow::Result<()> {
        if format == OutputFormat::Json {
            serde_json::to_writer(&mut self.stdout, page)?;
            writeln!(self.stdout)?;
        } else {
            for operation in &page.operations {
                writeln!(
                    self.stdout,
                    "{}: {:?} via {}",
                    operation.operation_id.0, operation.phase, operation.provider_pubkey.0,
                )?;
            }
        }
        Ok(())
    }

    fn rejections_human(
        &mut self,
        rejected: &[fa_client::RejectedAdvertisement],
    ) -> anyhow::Result<()> {
        if rejected.is_empty() {
            return Ok(());
        }
        writeln!(self.stdout, "rejections:")?;
        for rejection in rejected {
            writeln!(
                self.stdout,
                "  {}: {:?}",
                rejection.author, rejection.reason
            )?;
        }
        Ok(())
    }
}

fn rejections_json(rejected: &[fa_client::RejectedAdvertisement]) -> Vec<RejectionJson> {
    rejected
        .iter()
        .map(|rejection| RejectionJson {
            author: rejection.author.to_string(),
            reason: rejection.reason.code(),
        })
        .collect()
}

#[cfg(test)]
#[path = "output/tests.rs"]
mod tests;
