//! Durable MG-authored evidence, never a cached admission decision.

use std::{collections::BTreeMap, sync::Arc, time::Duration};

use fedi_decentralized_domain::{MG_TRUST_MATERIAL_MAX_AGE_SECS, ProtocolV1, Pubkey};
use fedi_decentralized_service_manifold_guardian::{
    GetMgTrustMaterialRequest, GetMgTrustMaterialResponse, ManifoldGuardianService, Timestamp,
};
use fedimint_core::{
    runtime::{sleep, timeout},
    task::TaskGroup,
};
use futures::{StreamExt as _, stream::FuturesUnordered};
use tokio::sync::watch;

use crate::{
    FaError, FaResult, FaStatus, Locator, ManifoldGuardianConnector, db::FaStore,
    liquidity::FA_LIQUIDITY_RPC_TIMEOUT,
};

/// Refresh when less than eight hours remain in the liquidity reader's window.
const REFRESH_AHEAD_SECS: u64 = 8 * 60 * 60;

/// Give deployed one-hour FLIP readers a fresh document when shopping starts.
pub(crate) async fn refresh_for_liquidity_shopping<F: ManifoldGuardianConnector>(
    store: &FaStore,
    connector: &F,
    status: &FaStatus,
) {
    // Temporary compatibility fetch for old FLIPs with a one-hour expiry policy.
    // Remove when those readers no longer need support; the normal cache policy
    // remains seven days. This is one concurrent batch, not a polling worker.
    const LEGACY_REFRESH_AGE_SECS: u64 = 30 * 60;
    let mgs = formed_mgs(status);
    let mut pending = FuturesUnordered::new();
    for (mg, locator) in &mgs {
        pending.push(async {
            if let Some(response) = cached(store, mg).await
                && now().0.saturating_sub(response.material.issued_at.0) > LEGACY_REFRESH_AGE_SECS
            {
                // A failed compatibility fetch must not hide usable offline
                // evidence or prevent the user from discovering providers.
                let _ = refresh(store, connector, mg, locator).await;
            }
        });
    }
    while pending.next().await.is_some() {}
}

/// Revalidate evidence before every use. A failed refresh may still use the
/// cached document while it satisfies the reader's policy. Admission stays live.
pub(crate) async fn acquire<F: ManifoldGuardianConnector>(
    store: &FaStore,
    connector: &F,
    mg: &Pubkey,
    locator: &Locator,
) -> FaResult<GetMgTrustMaterialResponse> {
    if let Some(response) = cached(store, mg).await
        && refresh_delay(&response) > Duration::ZERO
        && !store.mg_trust_material_refresh_required(&response).await
    {
        return Ok(response);
    }
    match refresh(store, connector, mg, locator).await {
        Ok(response) => Ok(response),
        Err(error) => cached(store, mg).await.ok_or(error),
    }
}

/// Cache validity never includes credential admission or issuer revocation.
async fn cached(store: &FaStore, mg: &Pubkey) -> Option<GetMgTrustMaterialResponse> {
    let response = store.mg_trust_material(&mg.0).await?;
    response
        .verify_for_mg_age(mg, now(), MG_TRUST_MATERIAL_MAX_AGE_SECS)
        .ok()?;
    Some(response)
}

/// One bounded read, independent of liquidity submission.
async fn refresh<F: ManifoldGuardianConnector>(
    store: &FaStore,
    connector: &F,
    mg: &Pubkey,
    locator: &Locator,
) -> FaResult<GetMgTrustMaterialResponse> {
    timeout(FA_LIQUIDITY_RPC_TIMEOUT, async {
        let client = connector
            .connect(locator)
            .await
            .map_err(|error| FaError::Liquidity(format!("connecting to federation MG: {error}")))?;
        let response = client
            .get_fman_trust_material(GetMgTrustMaterialRequest {
                version: ProtocolV1,
            })
            .await
            .map_err(|error| FaError::Liquidity(format!("fetching MG trust material: {error}")))?;
        response
            .verify_for_mg_age(mg, now(), MG_TRUST_MATERIAL_MAX_AGE_SECS)
            .map_err(|error| FaError::Liquidity(format!("verifying MG trust material: {error}")))?;
        store.cache_mg_trust_material(&response).await
    })
    .await
    .map_err(|_| FaError::Timeout("fetching MG trust material".to_owned()))?
}

fn now() -> Timestamp {
    Timestamp(fedimint_core::time::duration_since_epoch().as_secs())
}

/// Wake at the first second with strictly less than eight hours remaining.
fn refresh_delay(response: &GetMgTrustMaterialResponse) -> Duration {
    Duration::from_secs(
        response
            .material
            .issued_at
            .0
            .saturating_add(MG_TRUST_MATERIAL_MAX_AGE_SECS - REFRESH_AHEAD_SECS + 1)
            .saturating_sub(now().0),
    )
}

/// Maintain one identity even while the client is idle. Failures retain cached
/// evidence and leave foreground acquisition available.
async fn maintain<F: ManifoldGuardianConnector>(
    store: &FaStore,
    connector: &F,
    mg: &Pubkey,
    locator: &Locator,
) -> FaResult<()> {
    loop {
        if let Some(response) = cached(store, mg).await {
            let delay = refresh_delay(&response);
            if !delay.is_zero() && !store.mg_trust_material_refresh_required(&response).await {
                sleep(delay).await;
                continue;
            }
        }
        let response = refresh(store, connector, mg, locator).await?;
        // A peer can replay valid but almost-stale evidence. Do not spin on it.
        if refresh_delay(&response).is_zero()
            || store.mg_trust_material_refresh_required(&response).await
        {
            return Err(FaError::Liquidity(
                "MG returned evidence still requiring refresh".to_owned(),
            ));
        }
    }
}

/// Prefetch on formation/reopen/MG-set change, then sleep to each reader-policy
/// refresh deadline. No formation lease is held and success is never delayed.
/// Sharing only the connector and store lets the last client drop cancel it.
pub(crate) fn spawn<F>(
    tasks: &TaskGroup,
    store: FaStore,
    connector: Arc<F>,
    mut progress: watch::Receiver<FaStatus>,
) where
    F: ManifoldGuardianConnector,
{
    tasks.spawn_cancellable("FA MG trust prefetch", async move {
        let mut mgs = formed_mgs(&progress.borrow_and_update());
        loop {
            let mut pending = FuturesUnordered::new();
            for (mg, locator) in &mgs {
                pending.push(maintain(&store, connector.as_ref(), mg, locator));
            }
            let next = loop {
                tokio::select! {
                    result = pending.next(), if !pending.is_empty() => {
                        if let Some(Err(error)) = result {
                            tracing::debug!(%error, "opportunistic MG trust prefetch failed");
                        }
                    }
                    changed = progress.changed() => {
                        if changed.is_err() { return; }
                        let next = formed_mgs(&progress.borrow_and_update());
                        if next != mgs { break next; }
                    }
                }
            };
            // Cancel in-flight reads and deadline sleeps before using a new set.
            drop(pending);
            mgs = next;
        }
    });
}

fn formed_mgs(status: &FaStatus) -> BTreeMap<Pubkey, Locator> {
    let mut mgs = BTreeMap::new();
    if let FaStatus::Formed(snapshot) = status {
        for seat in &snapshot.seats {
            mgs.entry(Pubkey(seat.mg_identity.to_string()))
                .or_insert_with(|| seat.locator.clone());
        }
    }
    mgs
}
