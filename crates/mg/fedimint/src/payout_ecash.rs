//! Native ecash payout: spend a whole wallet into one bearer token under the
//! operation id its request names, read the token back from that operation,
//! and reclaim it.
//!
//! The semantics follow the Fedi app's no-timeout ecash send and cancel. The
//! spend registers no auto-refund state machine; the token lives in the spend
//! operation's metadata, committed in the same wallet transaction that removes
//! the notes, so it is never lost and never stored twice. A cancel reissues
//! those same notes into this wallet under a deterministic operation id; the
//! federation refusing that reissue is how the wallet learns the recipient
//! spent the notes first. Mint v1 and mint v2 differ only in the module calls.

use anyhow::Context as _;
use fedimint_client::ClientHandleArc;
use fedimint_core::core::OperationId;
use fedimint_core::{Amount, TieredMulti};
use fedimint_mint_client::{
    MintClientModule, MintOperationMeta, MintOperationMetaVariant, NotesSelector, OOBNotes,
    ReissueExternalNotesState,
};
use fedimint_mint_common::config::FeeConsensus;
use fedimint_mintv2_client::{
    ECash as MintV2Ecash, FinalReceiveOperationState as MintV2ReceiveState,
    MintClientModule as MintV2ClientModule, MintOperationMeta as MintV2OperationMeta,
    ReceiveECashError,
};
use futures::StreamExt as _;
use tracing::info;

use crate::payout_job::{Payout, PayoutRequestId};
use crate::payout_operation_id::PayoutOperationId;

/// Bound on a mint-v2 send that needs a reissue round first. A whole-balance
/// send spends exact notes locally, so this only matters when notes arrived
/// between reading the balance and spending it.
const MINT_V2_SEND_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// A bearer ecash token. Whoever holds the string can redeem the notes, so it
/// has no `Display`, `Debug` redacts it, and it reaches only the operator.
#[derive(Clone, Eq, PartialEq)]
pub struct EcashToken(String);

#[cfg(test)]
impl EcashToken {
    pub(crate) fn test(token: &str) -> Self {
        Self(token.to_owned())
    }
}

impl std::fmt::Debug for EcashToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("EcashToken(<redacted>)")
    }
}

/// Where an ecash payout's notes are. The wallet database decides every
/// state; the operator's confirmation is a separate fact of the job.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EcashPayoutState {
    Issued { token: EcashToken },
    Reclaiming,
    Reclaimed,
    AlreadySpent,
}

impl EcashPayoutState {
    /// The wire state, carrying an issued payout's token only `with_token`.
    pub(crate) fn to_wire(&self, with_token: bool) -> mg_core::payout_wire::EcashPayoutStateWire {
        use mg_core::payout_wire::{EcashPayoutStateWire, EcashTokenWire};
        match self {
            Self::Issued { token } => EcashPayoutStateWire::Issued {
                token: with_token.then(|| EcashTokenWire(token.0.clone())),
            },
            Self::Reclaiming => EcashPayoutStateWire::Reclaiming,
            Self::Reclaimed => EcashPayoutStateWire::Reclaimed,
            Self::AlreadySpent => EcashPayoutStateWire::AlreadySpent,
        }
    }

    /// Whether the notes are not settled yet: a token that can still be
    /// redeemed, or a reclaim to finish.
    pub(crate) fn is_open(&self) -> bool {
        matches!(self, Self::Issued { .. } | Self::Reclaiming)
    }
}

/// Selects every spendable note, whatever amount is requested. Selection runs
/// inside the spend's own database transaction, so "the whole balance" is
/// exactly the notes that transaction removes.
struct AllNotes;

#[async_trait::async_trait]
impl<Note: Send> NotesSelector<Note> for AllNotes {
    async fn select_notes(
        &self,
        stream: impl futures::Stream<Item = (Amount, Note)> + Send,
        _requested_amount: Amount,
        _fee_consensus: FeeConsensus,
    ) -> anyhow::Result<TieredMulti<Note>> {
        let notes: TieredMulti<Note> = stream.collect::<Vec<_>>().await.into_iter().collect();
        anyhow::ensure!(notes.count_items() > 0, "no balance to pay out");
        Ok(notes)
    }
}

/// Which mint holds this client's Bitcoin balance.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MintGeneration {
    V1,
    V2,
}

impl MintGeneration {
    fn of(client: &ClientHandleArc) -> anyhow::Result<Self> {
        let (primary, _) = client
            .primary_module_for_unit(fedimint_core::module::AmountUnit::BITCOIN)
            .context("federation has no Bitcoin mint")?;
        if client
            .get_first_module::<MintV2ClientModule>()
            .is_ok_and(|mint| mint.id == primary)
        {
            Ok(Self::V2)
        } else if client
            .get_first_module::<MintClientModule>()
            .is_ok_and(|mint| mint.id == primary)
        {
            Ok(Self::V1)
        } else {
            anyhow::bail!("the federation's Bitcoin balance is not held by a supported mint")
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::V1 => "mint",
            Self::V2 => "mintv2",
        }
    }
}

/// Spend the whole balance into one token carrying the federation invite,
/// under the operation id the request names, and return after the spend, its
/// metadata and the token are committed. A spend already recorded under that
/// id is this payout, found instead of repeated.
pub(crate) async fn start_ecash_payout(
    client: &ClientHandleArc,
    request_id: &PayoutRequestId,
) -> anyhow::Result<Payout> {
    let operation_id = PayoutOperationId::ecash(request_id);
    if let Some(payout) = find_ecash_payout(client, &operation_id).await? {
        return Ok(payout);
    }
    let generation = MintGeneration::of(client)?;
    let amount = match generation {
        MintGeneration::V1 => {
            let mint = client.get_first_module::<MintClientModule>()?;
            let balance = client.get_balance_for_btc().await?;
            anyhow::ensure!(balance > Amount::ZERO, "no balance to pay out");
            mint.spend_notes_with_selector_and_operation_id(
                operation_id.to_native(),
                &AllNotes,
                balance,
                None,
                true,
                payout_metadata(request_id),
            )
            .await?
            .total_amount()
        }
        MintGeneration::V2 => {
            let mint = client.get_first_module::<MintV2ClientModule>()?;
            let balance = mint
                .get_count_by_denomination()
                .await
                .into_iter()
                .try_fold(0u64, |total, (denomination, count)| {
                    denomination
                        .amount()
                        .msats
                        .checked_mul(count)
                        .and_then(|value| total.checked_add(value))
                })
                .context("mint-v2 balance overflows millisatoshis")?;
            anyhow::ensure!(balance > 0, "no balance to pay out");
            tokio::time::timeout(
                MINT_V2_SEND_TIMEOUT,
                mint.send_with_operation_id(
                    operation_id.to_native(),
                    Amount::from_msats(balance),
                    payout_metadata(request_id),
                    true,
                ),
            )
            .await
            .context("mint-v2 ecash send timed out")?
            .map_err(|error| anyhow::anyhow!("mint-v2 ecash send failed: {error}"))?
            .amount()
        }
    };
    let payout = Payout {
        operation_id,
        amount_msat: amount.msats,
    };
    info!(
        federation_id = %client.federation_id(),
        mint = generation.name(),
        operation_id = %payout.operation_id,
        amount_msat = payout.amount_msat,
        "committed MG ecash payout"
    );
    Ok(payout)
}

/// The ecash payout recorded under `operation_id`, if its spend exists.
pub(crate) async fn find_ecash_payout(
    client: &ClientHandleArc,
    operation_id: &PayoutOperationId,
) -> anyhow::Result<Option<Payout>> {
    Ok(spent_notes(client, operation_id)
        .await?
        .map(|notes| Payout {
            operation_id: operation_id.clone(),
            amount_msat: notes.amount().msats,
        }))
}

/// Read where the notes of one committed ecash payout are. With `wait`, a
/// submitted reclaim is awaited to its terminal answer; without it, a reclaim
/// whose state machines are still running reads as `Reclaiming`.
pub(crate) async fn ecash_payout_state(
    client: &ClientHandleArc,
    operation_id: &PayoutOperationId,
    wait: bool,
) -> anyhow::Result<EcashPayoutState> {
    match committed_spend(client, operation_id).await? {
        SpentNotes::V1(notes) => {
            let reclaim_id = crate::reissue_operation_id(&notes);
            let Some(reclaim) = client.operation_log().get_operation(reclaim_id).await else {
                return Ok(EcashPayoutState::Issued {
                    token: EcashToken(notes.to_string()),
                });
            };
            if let Some(state) = reclaim
                .try_outcome::<ReissueExternalNotesState>()
                .context("decode ecash reclaim outcome")?
                .and_then(v1_reclaim_state)
            {
                return Ok(state);
            }
            if !wait && client.has_active_states(reclaim_id).await {
                return Ok(EcashPayoutState::Reclaiming);
            }
            let mut updates = client
                .get_first_module::<MintClientModule>()?
                .subscribe_reissue_external_notes(reclaim_id)
                .await
                .context("subscribe to ecash reclaim")?
                .into_stream();
            let mut last = None;
            while let Some(update) = updates.next().await {
                last = Some(update);
            }
            last.and_then(v1_reclaim_state)
                .context("ecash reclaim updates ended before a terminal state")
        }
        SpentNotes::V2(ecash) => {
            let reclaim_id = OperationId::from_encodable(&ecash);
            let Some(reclaim) = client.operation_log().get_operation(reclaim_id).await else {
                return Ok(EcashPayoutState::Issued {
                    token: EcashToken(fedimint_core::base32::encode_prefixed(
                        fedimint_core::base32::FEDIMINT_PREFIX,
                        &ecash,
                    )),
                });
            };
            let outcome = match reclaim
                .try_outcome::<MintV2ReceiveState>()
                .context("decode mint-v2 ecash reclaim outcome")?
            {
                Some(outcome) => outcome,
                None if !wait && client.has_active_states(reclaim_id).await => {
                    return Ok(EcashPayoutState::Reclaiming);
                }
                None => client
                    .get_first_module::<MintV2ClientModule>()?
                    .await_final_receive_operation_state(reclaim_id)
                    .await
                    .context("await mint-v2 ecash reclaim")?,
            };
            // A mint-v2 receive succeeds once its transaction is accepted, but
            // the reissued notes reach the balance only when its outputs are
            // signed. Mint v1's `Done` already waits for them.
            if outcome == MintV2ReceiveState::Success {
                if !wait {
                    if client.has_active_states(reclaim_id).await {
                        return Ok(EcashPayoutState::Reclaiming);
                    }
                } else {
                    let MintV2OperationMeta::Receive {
                        change_outpoint_range,
                        ..
                    } = reclaim
                        .try_meta::<MintV2OperationMeta>()
                        .context("decode mint-v2 ecash reclaim metadata")?
                    else {
                        anyhow::bail!("mint-v2 ecash reclaim is not a receive operation");
                    };
                    client
                        .await_primary_bitcoin_module_outputs(
                            reclaim_id,
                            change_outpoint_range.into_iter().collect(),
                        )
                        .await
                        .context("await mint-v2 ecash reclaim notes")?;
                }
            }
            Ok(v2_reclaim_state(outcome))
        }
    }
}

/// Submit the reissue of one payout's notes back into this wallet, or find the
/// one already submitted. Its operation id derives from the notes, so a retry
/// can never submit a second reclaim.
pub(crate) async fn start_ecash_reclaim(
    client: &ClientHandleArc,
    request_id: &PayoutRequestId,
    operation_id: &PayoutOperationId,
) -> anyhow::Result<()> {
    let metadata = serde_json::json!({
        "purpose": "fman-payout-reclaim",
        "request_id": request_id,
    });
    let reclaim_id = match committed_spend(client, operation_id).await? {
        SpentNotes::V1(notes) => {
            let reclaim_id = crate::reissue_operation_id(&notes);
            if let Err(error) = client
                .get_first_module::<MintClientModule>()?
                .reissue_external_notes(notes, metadata)
                .await
            {
                // The pinned client refuses a reissue whose deterministic
                // operation already exists: that is this reclaim, resumed.
                anyhow::ensure!(
                    client.operation_exists(reclaim_id).await,
                    "reclaim ecash payout: {error}"
                );
            }
            reclaim_id
        }
        SpentNotes::V2(ecash) => {
            let reclaim_id = OperationId::from_encodable(&ecash);
            match client
                .get_first_module::<MintV2ClientModule>()?
                .receive(ecash, metadata)
                .await
            {
                Ok(_) | Err(ReceiveECashError::AlreadyReceived) => {}
                Err(error) => anyhow::bail!("reclaim mint-v2 ecash payout: {error}"),
            }
            reclaim_id
        }
    };
    info!(
        federation_id = %client.federation_id(),
        operation_id = %operation_id,
        reclaim_operation_id = %reclaim_id.fmt_full(),
        "submitted MG ecash payout reclaim"
    );
    Ok(())
}

/// The notes one ecash payout spent.
enum SpentNotes {
    V1(OOBNotes),
    V2(MintV2Ecash),
}

impl SpentNotes {
    fn amount(&self) -> Amount {
        match self {
            Self::V1(notes) => notes.total_amount(),
            Self::V2(ecash) => ecash.amount(),
        }
    }
}

/// The notes of the committed spend under `operation_id`.
async fn committed_spend(
    client: &ClientHandleArc,
    operation_id: &PayoutOperationId,
) -> anyhow::Result<SpentNotes> {
    spent_notes(client, operation_id)
        .await?
        .context("native ecash payout operation does not exist")
}

/// Read the spend operation under `operation_id`, which must be an ecash
/// spend: the ledger chose the id, so anything else there is corruption.
async fn spent_notes(
    client: &ClientHandleArc,
    operation_id: &PayoutOperationId,
) -> anyhow::Result<Option<SpentNotes>> {
    let Some(entry) = client
        .operation_log()
        .get_operation(operation_id.to_native())
        .await
    else {
        return Ok(None);
    };
    Ok(Some(match entry.operation_module_kind() {
        "mint" => {
            let MintOperationMetaVariant::SpendOOB { oob_notes, .. } = entry
                .try_meta::<MintOperationMeta>()
                .context("decode mint operation metadata")?
                .variant
            else {
                anyhow::bail!("ecash payout operation is not an out-of-band spend");
            };
            SpentNotes::V1(oob_notes)
        }
        "mintv2" => {
            let MintV2OperationMeta::Send { ecash, .. } =
                entry
                    .try_meta::<MintV2OperationMeta>()
                    .context("decode mint-v2 operation metadata")?
            else {
                anyhow::bail!("ecash payout operation is not a mint-v2 send");
            };
            SpentNotes::V2(
                fedimint_core::base32::decode_prefixed(
                    fedimint_core::base32::FEDIMINT_PREFIX,
                    &ecash,
                )
                .context("decode mint-v2 ecash payout")?,
            )
        }
        kind => anyhow::bail!("ecash payout operation belongs to module {kind}"),
    }))
}

fn payout_metadata(request_id: &PayoutRequestId) -> serde_json::Value {
    serde_json::json!({
        "purpose": "fman-payout",
        "request_id": request_id,
    })
}

/// A mint-v1 reissue rejected by the federation means the notes' inputs were
/// already spent: the recipient redeemed the token first.
fn v1_reclaim_state(state: ReissueExternalNotesState) -> Option<EcashPayoutState> {
    match state {
        ReissueExternalNotesState::Done => Some(EcashPayoutState::Reclaimed),
        ReissueExternalNotesState::Failed(_) => Some(EcashPayoutState::AlreadySpent),
        ReissueExternalNotesState::Created | ReissueExternalNotesState::Issuing => None,
    }
}

fn v2_reclaim_state(state: MintV2ReceiveState) -> EcashPayoutState {
    match state {
        MintV2ReceiveState::Success => EcashPayoutState::Reclaimed,
        MintV2ReceiveState::Rejected => EcashPayoutState::AlreadySpent,
    }
}

#[cfg(test)]
mod tests;
