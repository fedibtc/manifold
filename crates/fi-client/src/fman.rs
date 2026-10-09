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

    fn timeout(&self, operation: FmanOperation, error: FiError) -> FiError {
        match error {
            FiError::Timeout(_) => self.failure(operation, FmanCause::Timeout).into(),
            error => error,
        }
    }

    async fn request<T, Fut: Future<Output = FmResult<T>>>(
        &self,
        run: DriverRun<'_>,
        operation: FmanOperation,
        make_future: impl FnOnce() -> Fut,
    ) -> FiResult<T> {
        run.call(operation.as_str(), || Ok(make_future()))
            .await
            .map_err(|error| self.timeout(operation, error))?
            .map_err(|error| self.failure(operation, error.into()).into())
    }

    pub(crate) async fn get_availability<F: FleetManagerConnector<Client = C>>(
        &self,
        connector: &F,
        run: DriverRun<'_>,
        request: GetAvailabilityRequest,
    ) -> FiResult<GetAvailabilityResponse> {
        let operation = FmanOperation::GetAvailability;
        run.call(operation.as_str(), || {
            Ok(connector.get_availability(&self.inner, request))
        })
        .await
        .map_err(|error| self.timeout(operation, error))?
        .map_err(|_| self.failure(operation, FmanCause::Transport))?
        .map_err(|error| self.failure(operation, error.into()).into())
    }

    pub(crate) async fn get_quote<F: FleetManagerConnector<Client = C>>(
        &self,
        connector: &F,
        run: DriverRun<'_>,
        request: GetQuoteRequest,
    ) -> FiResult<SignedResponse<GetQuoteResponse>> {
        let operation = FmanOperation::GetQuote;
        run.call(operation.as_str(), || {
            Ok(connector.get_quote(&self.inner, request))
        })
        .await
        .map_err(|error| self.timeout(operation, error))?
        .map_err(|_| self.failure(operation, FmanCause::Transport))?
        .map_err(|error| self.failure(operation, error.into()).into())
    }

    pub(crate) async fn create_seat(
        &self,
        run: DriverRun<'_>,
        request: SignedRequest<CreateSeatRequest>,
    ) -> FiResult<SignedResponse<CreateSeatResponse>> {
        self.request(run, FmanOperation::CreateSeat, || {
            self.inner.create_seat(request)
        })
        .await
    }

    pub(crate) async fn get_dkg_code(
        &self,
        run: DriverRun<'_>,
        request: SignedRequest<GetDkgCodeRequest>,
    ) -> FiResult<GetDkgCodeResponse> {
        self.request(run, FmanOperation::GetDkgCode, || {
            self.inner.get_dkg_code(request)
        })
        .await
    }

    pub(crate) async fn start_dkg(
        &self,
        run: DriverRun<'_>,
        request: SignedRequest<StartDkgRequest>,
    ) -> FiResult<StartDkgResponse> {
        self.request(run, FmanOperation::StartDkg, || {
            self.inner.start_dkg(request)
        })
        .await
    }

    pub(crate) async fn restart_dkg(
        &self,
        run: DriverRun<'_>,
        request: SignedRequest<RestartDkgRequest>,
    ) -> FiResult<RestartDkgResponse> {
        self.request(run, FmanOperation::RestartDkg, || {
            self.inner.restart_dkg(request)
        })
        .await
    }

    pub(crate) async fn get_status(
        &self,
        run: DriverRun<'_>,
        request: SignedRequest<GetStatusRequest>,
    ) -> FiResult<GetStatusResponse> {
        self.request(run, FmanOperation::GetStatus, || {
            self.inner.get_status(request)
        })
        .await
    }

    pub(crate) async fn get_invite_code(
        &self,
        run: DriverRun<'_>,
        request: SignedRequest<GetInviteCodeRequest>,
    ) -> FiResult<GetInviteCodeResponse> {
        self.request(run, FmanOperation::GetInviteCode, || {
            self.inner.get_invite_code(request)
        })
        .await
    }

    pub(crate) async fn get_peer_attestation(
        &self,
        run: DriverRun<'_>,
        request: SignedRequest<GetPeerAttestationRequest>,
    ) -> FiResult<GetPeerAttestationResponse> {
        self.request(run, FmanOperation::GetPeerAttestation, || {
            self.inner.get_peer_attestation(request)
        })
        .await
    }

    pub(crate) async fn propose_formation_meta(
        &self,
        run: DriverRun<'_>,
        request: SignedRequest<ProposeFormationMetaRequest>,
    ) -> FiResult<ProposeFormationMetaResponse> {
        self.request(run, FmanOperation::ProposeFormationMeta, || {
            self.inner.propose_formation_meta(request)
        })
        .await
    }
}
