//! Identity-bound formation requests. Wire clients and connector ports stay unchanged.

use std::future::Future;

use fedi_decentralized_service_manifold_guardian::*;

use crate::formation::DriverRun;
use crate::{FaError, FaResult, ManifoldGuardianConnector, MgCause, MgFailure, MgOperation};

pub(crate) struct MgClient<C> {
    pub(crate) mg: secp256k1::XOnlyPublicKey,
    inner: C,
}

impl<C: ManifoldGuardianService> MgClient<C> {
    pub(crate) async fn connect<F: ManifoldGuardianConnector<Client = C>>(
        connector: &F,
        locator: &Locator,
        run: DriverRun,
    ) -> FaResult<Self> {
        let failure = |cause| MgFailure {
            mg: locator.service_pubkey,
            operation: MgOperation::Connect,
            cause,
        };
        let inner = run
            .call(MgOperation::Connect.as_str(), || {
                Ok(connector.connect(locator))
            })
            .await
            .map_err(|error| match error {
                FaError::Timeout(_) => FaError::Mg(failure(MgCause::Timeout)),
                error => error,
            })?
            .map_err(|_| failure(MgCause::Transport))?;
        Ok(Self {
            mg: locator.service_pubkey,
            inner,
        })
    }

    pub(crate) fn failure(&self, operation: MgOperation, cause: MgCause) -> MgFailure {
        MgFailure {
            mg: self.mg,
            operation,
            cause,
        }
    }

    /// Construct the service future only after the driver's ownership fence.
    pub(crate) async fn call<'s, T, E, Fut>(
        &'s self,
        run: DriverRun,
        operation: MgOperation,
        request: impl FnOnce(&'s C) -> Fut,
    ) -> FaResult<T>
    where
        Fut: Future<Output = Result<T, E>> + 's,
        E: Into<MgCause>,
    {
        run.call(operation.as_str(), || Ok(request(&self.inner)))
            .await
            .map_err(|error| match error {
                FaError::Timeout(_) => self.failure(operation, MgCause::Timeout).into(),
                error => error,
            })?
            .map_err(|error| self.failure(operation, error.into()).into())
    }
}

/// Preserve the connector's distinction between transport and service refusals.
pub(crate) fn fold_transport<T, E>(result: Result<FmResult<T>, E>) -> Result<T, MgCause> {
    result.map_err(|_| MgCause::Transport)?.map_err(Into::into)
}
