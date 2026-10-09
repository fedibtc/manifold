//! Identity-bound formation requests. Wire clients and connector ports stay unchanged.

use std::future::Future;

use fedi_decentralized_service_fleet_manager::*;

use crate::formation::DriverRun;
use crate::{FiError, FiResult, FleetManagerConnector, FmanCause, FmanFailure, FmanOperation};

pub(crate) struct FmanClient<C> {
    pub(crate) fman: secp256k1::XOnlyPublicKey,
    inner: C,
}

impl<C: FleetManagerService> FmanClient<C> {
    pub(crate) async fn connect<F: FleetManagerConnector<Client = C>>(
        connector: &F,
        locator: &Locator,
        run: DriverRun<'_>,
    ) -> FiResult<Self> {
        let failure = |cause| FmanFailure {
            fman: locator.service_pubkey,
            operation: FmanOperation::Connect,
            cause,
        };
        let inner = run
            .call(FmanOperation::Connect.as_str(), || {
                Ok(connector.connect(locator))
            })
            .await
            .map_err(|error| match error {
                FiError::Timeout(_) => FiError::Fman(failure(FmanCause::Timeout)),
                error => error,
            })?
            .map_err(|_| failure(FmanCause::Transport))?;
        Ok(Self {
            fman: locator.service_pubkey,
            inner,
        })
    }

    pub(crate) fn failure(&self, operation: FmanOperation, cause: FmanCause) -> FmanFailure {
        FmanFailure {
            fman: self.fman,
            operation,
            cause,
        }
    }

    /// Construct the service future only after the driver's ownership fence.
    pub(crate) async fn call<'s, T, E, Fut>(
        &'s self,
        run: DriverRun<'_>,
        operation: FmanOperation,
        request: impl FnOnce(&'s C) -> Fut,
    ) -> FiResult<T>
    where
        Fut: Future<Output = Result<T, E>> + 's,
        E: Into<FmanCause>,
    {
        run.call(operation.as_str(), || Ok(request(&self.inner)))
            .await
            .map_err(|error| match error {
                FiError::Timeout(_) => self.failure(operation, FmanCause::Timeout).into(),
                error => error,
            })?
            .map_err(|error| self.failure(operation, error.into()).into())
    }
}

/// Preserve the connector's distinction between transport and service refusals.
pub(crate) fn fold_transport<T, E>(result: Result<FmResult<T>, E>) -> Result<T, FmanCause> {
    result
        .map_err(|_| FmanCause::Transport)?
        .map_err(Into::into)
}
