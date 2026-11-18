//! FA-supplied custom gateway attachment.
//!
//! In v1 a federation gets one gateway source: either FLIP liquidity
//! (`liquidity`) or one gateway the FA names here, which a provider runs and
//! funds outside Manifold. The contract is
//! `specs/SPEC-fa-post-formation-liquidity.md`.

use fedi_decentralized_domain::{FederationId, GatewayApiUrl};
use fedi_decentralized_nostr_clients::FaNostrClient;

use crate::formation::{DriverRun, finish_driver_run, invite_federation_id, start_driver_run};
use crate::{
    FaClient, FaError, FaIdentity, FaPayments, FaResult, FederationConsensusReader,
    MaintenanceRunOptions, ManifoldGuardianConnector,
};

/// Durable phase of the federation's custom gateway attachment.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CustomGatewayPhase {
    /// Recorded before the first `RegisterGateway`; the fresh LNv2 view has
    /// not yet been seen to contain the gateway. Resume it with the same URL.
    Attaching,
    /// A threshold of guardians stored the gateway and the fresh LNv2 view
    /// contains it. Final in v1.
    Attached,
}

/// The FA-supplied gateway of the formed federation.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct CustomGatewayAttachment {
    pub gateway_api: GatewayApiUrl,
    pub phase: CustomGatewayPhase,
}

/// Persisted attachment, keyed by federation id.
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub(crate) struct StoredCustomGateway {
    pub(crate) gateway_api: GatewayApiUrl,
    /// Whether the fresh LNv2 view was seen to contain `gateway_api`.
    pub(crate) attached: bool,
}

impl StoredCustomGateway {
    fn snapshot(&self) -> CustomGatewayAttachment {
        CustomGatewayAttachment {
            gateway_api: self.gateway_api.clone(),
            phase: if self.attached {
                CustomGatewayPhase::Attached
            } else {
                CustomGatewayPhase::Attaching
            },
        }
    }
}

impl<I, P, N, F, C> FaClient<I, P, N, F, C>
where
    I: FaIdentity,
    P: FaPayments,
    N: FaNostrClient,
    F: ManifoldGuardianConnector,
    C: FederationConsensusReader,
{
    /// Attach an FA-supplied LNv2 gateway to the formed federation.
    ///
    /// The attachment is recorded before the first guardian call, then uses
    /// the same threshold `RegisterGateway` fan-out and fresh LNv2 readback as
    /// a FLIP gateway. After a failure, crash or cancellation, calling again
    /// with the same URL resumes it; [`Self::custom_gateway_attachment`]
    /// returns the unfinished URL. A federation gets one gateway source:
    ///
    /// - a liquidity operation that is live or completed refuses this with
    ///   [`FaError::LiquidityOperationExists`];
    /// - another URL, while an attachment is unfinished or after one
    ///   completed, is refused with [`FaError::CustomGatewayExists`];
    /// - the same URL after completion returns the attachment without
    ///   network work.
    pub async fn attach_custom_gateway(
        &self,
        gateway_api: GatewayApiUrl,
        options: MaintenanceRunOptions,
    ) -> FaResult<CustomGatewayAttachment> {
        let _guard = self.inner.run_guard.try_lock().map_err(|_| FaError::Busy)?;
        let options = options.0;
        options.validate_for_start(&self.inner.store)?;
        let fa_id = self.fa_id()?;
        let (deadline, lease) = start_driver_run(&self.inner.store, options).await?;
        let result = async {
            let authority = self.maintenance_authority(fa_id).await?;
            let federation_id =
                FederationId(invite_federation_id(&authority.invite_code)?.to_string());
            match self.inner.store.load_custom_gateway(&federation_id).await {
                Some(stored) if stored.gateway_api != gateway_api || stored.attached => {
                    return if stored.gateway_api == gateway_api {
                        Ok(stored.snapshot())
                    } else {
                        Err(FaError::CustomGatewayExists {
                            attachment: stored.snapshot(),
                        })
                    };
                }
                Some(_) => {}
                None => {
                    if let Some(operation) = self
                        .find_live_liquidity_operation(|operation| {
                            operation.commitment.federation_details.federation_id == federation_id
                        })
                        .await?
                    {
                        return Err(FaError::LiquidityOperationExists {
                            operation_id: operation.operation_id,
                        });
                    }
                    lease.renew().await?;
                    self.inner
                        .store
                        .begin_custom_gateway(&federation_id, &gateway_api)
                        .await?;
                }
            }
            self.register_gateway_pinned(
                gateway_api.clone(),
                fa_id,
                DriverRun::new(options, deadline, &lease),
            )
            .await?;
            lease.renew().await?;
            self.inner
                .store
                .mark_custom_gateway_attached(&federation_id, &gateway_api)
                .await?;
            Ok(CustomGatewayAttachment {
                gateway_api,
                phase: CustomGatewayPhase::Attached,
            })
        }
        .await;
        finish_driver_run(result, self.inner.store.release_driver_lease(lease).await)
    }

    /// The formed federation's custom gateway attachment, if any.
    ///
    /// Read-only: no guardian or consensus work. `None` when no federation is
    /// formed or it has no custom attachment.
    pub async fn custom_gateway_attachment(&self) -> FaResult<Option<CustomGatewayAttachment>> {
        let crate::db::FaRecovery::Formed(formed) =
            self.inner.store.load_recovery(self.fa_id()?).await?
        else {
            return Ok(None);
        };
        let federation_id =
            FederationId(invite_federation_id(&formed.federation_invite)?.to_string());
        Ok(self
            .inner
            .store
            .load_custom_gateway(&federation_id)
            .await
            .map(|stored| stored.snapshot()))
    }

    /// Refuse FLIP liquidity for a federation with a custom attachment,
    /// unfinished or completed.
    pub(crate) async fn ensure_no_custom_gateway(
        &self,
        federation_id: &FederationId,
    ) -> FaResult<()> {
        match self.inner.store.load_custom_gateway(federation_id).await {
            Some(stored) => Err(FaError::CustomGatewayExists {
                attachment: stored.snapshot(),
            }),
            None => Ok(()),
        }
    }
}
