//! Authenticated common-set refresh, durable high-water retention, and selection.

use std::collections::HashSet;

use fedi_decentralized_domain::DEFAULT_SETUP_PAYMENT_MIN_FEE_PPM;
use fedi_decentralized_nostr::setup_payment_federations::{
    admit_setup_payment_federations_event, restore_durably_admitted_setup_payment_federations_event,
};
use fedi_decentralized_nostr_clients::{FiNostrClient, SETUP_PAYMENT_FEDERATIONS_CANDIDATE_LIMIT};
use fedi_decentralized_service_fleet_manager::{FederationId, InviteCode};
use nostr_sdk::Timestamp;

use crate::formation::{DriverRun, finish_driver_run, start_driver_run};
use crate::{
    FederationConsensusReader, FiClient, FiError, FiIdentity, FiPayments, FiResult,
    FleetManagerConnector,
};

/// One authenticated setup-payment federation and the invite that joins it.
///
/// The publication carries invites, not ids: the id is derived from the invite
/// during admission ([`SPEC-setup-payment-federations`]). A consumer that has
/// not joined a member yet therefore needs the invite as well as the id, which
/// is what [`SPEC-setup-payment-federations`] means by the signed invite being
/// the member's join material.
///
/// [`SPEC-setup-payment-federations`]: ../../../specs/SPEC-setup-payment-federations.md
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdmittedSetupPaymentFederation {
    /// Canonical federation id derived from `invite_code` during admission.
    federation_id: FederationId,

    /// Signed public invite this member is joined with. Never carries an API
    /// bearer secret: admission rejects a publication whose invite does.
    invite_code: InviteCode,
}

impl AdmittedSetupPaymentFederation {
    /// Canonical federation id derived from the admitted invite.
    #[must_use]
    pub fn federation_id(&self) -> &FederationId {
        &self.federation_id
    }

    /// Signed public invite admitted as this member's join material.
    #[must_use]
    pub fn invite_code(&self) -> &InviteCode {
        &self.invite_code
    }
}

impl<I, P, N, F, C> FiClient<I, P, N, F, C>
where
    I: FiIdentity,
    P: FiPayments,
    N: FiNostrClient,
    F: FleetManagerConnector,
    C: FederationConsensusReader,
{
    /// Return the authenticated setup-payment policy set in canonical order,
    /// each member paired with the signed invite that joins it.
    ///
    /// This includes admitted federations whose joined wallet currently has
    /// zero balance. Consumers can intersect the members with their own joined
    /// wallet projection to render payer/refill choices; Pay-and-create still
    /// validates the selected payer through [`FiPayments::payable_federations`]
    /// and the exact aggregate funding preflight.
    ///
    /// A consumer needs the invite to offer a member it has **not** joined: an
    /// id cannot be turned back into an invite.
    pub async fn admitted_setup_payment_federations(
        &self,
        options: crate::FormationRunOptions,
    ) -> FiResult<Vec<AdmittedSetupPaymentFederation>> {
        let _run = self.inner.run_guard.try_lock().map_err(|_| FiError::Busy)?;
        options.validate_for_start(&self.inner.store)?;
        let (deadline, lease) = start_driver_run(&self.inner.store, options).await?;
        let run = DriverRun::new(options, deadline, &lease);
        let result = self.refresh_setup_payment_federations(run).await;
        finish_driver_run(result, self.inner.store.release_driver_lease(lease).await)
    }

    /// Whether this FI is configured to pay at all.
    ///
    /// Paying needs a deployment-pinned setup-payment publisher: without one
    /// there is no authenticated set of federations to fund from, so the FI
    /// can only form seats an FMan gives away (a quote priced at zero). This
    /// is what makes the first federation in a deployment formable, before
    /// any ecash to pay with exists.
    pub(crate) fn can_pay(&self) -> bool {
        self.inner.setup_payment_publisher.is_some()
    }

    /// The smallest guardian fee rate this FI may propose right now, in ppm.
    ///
    /// Reads the durably admitted publication rather than refreshing it: the
    /// floor gates a local intent before any guardian is contacted, so it has
    /// to hold with the relays unreachable. Fails safe upwards exactly like
    /// the FMan-side resolution — a deployment with no pinned publisher, no
    /// stored event, or a stored event that no longer authenticates gets
    /// [`DEFAULT_SETUP_PAYMENT_MIN_FEE_PPM`], never zero. Only a publication
    /// Fedi actually signed can lower it below that default.
    pub(crate) async fn min_guardian_fee_ppm(&self) -> u64 {
        let Some(publisher) = self.inner.setup_payment_publisher else {
            return DEFAULT_SETUP_PAYMENT_MIN_FEE_PPM;
        };
        self.inner
            .store
            .load_setup_payment_federations_event()
            .await
            .and_then(|event| {
                restore_durably_admitted_setup_payment_federations_event(&event, publisher).ok()
            })
            .map_or(DEFAULT_SETUP_PAYMENT_MIN_FEE_PPM, |admitted| {
                admitted.set().min_fee_ppm()
            })
    }

    /// Refresh authenticated policy and select the canonical first
    /// wallet-capable member for the lower-level pinned formation path.
    pub(crate) async fn select_setup_payment_federation(
        &self,
        run: DriverRun<'_>,
    ) -> FiResult<FederationId> {
        let (policy_ids, payable) = self.payable_setup_payment_federations(run).await?;
        policy_ids
            .into_iter()
            .find(|federation_id| payable.contains(federation_id))
            .ok_or_else(|| {
                FiError::Payment(
                    "the consumer wallet can pay from no authenticated setup-payment federation"
                        .to_owned(),
                )
            })
    }

    /// Validate one consumer-selected payer against authenticated policy and
    /// current wallet readiness without changing the caller's choice.
    pub(crate) async fn require_setup_payment_federation(
        &self,
        requested: &FederationId,
        run: DriverRun<'_>,
    ) -> FiResult<FederationId> {
        let (policy_ids, payable) =
            self.payable_setup_payment_federations(run)
                .await
                .map_err(|error| match error {
                    FiError::Payment(_) => FiError::SelectionReauthorizationRequired(
                        crate::SelectionReauthorizationReason::SelectedPayerUnavailable,
                    ),
                    error => error,
                })?;
        if !policy_ids.contains(requested) || !payable.contains(requested) {
            return Err(FiError::SelectionReauthorizationRequired(
                crate::SelectionReauthorizationReason::SelectedPayerUnavailable,
            ));
        }
        Ok(requested.clone())
    }

    async fn payable_setup_payment_federations(
        &self,
        run: DriverRun<'_>,
    ) -> FiResult<(Vec<FederationId>, HashSet<FederationId>)> {
        let policy_ids = self
            .refresh_setup_payment_federations(run)
            .await?
            .into_iter()
            .map(|member| member.federation_id)
            .collect::<Vec<_>>();
        if policy_ids.is_empty() {
            return Err(FiError::Payment(
                "the authenticated setup-payment federation set is empty".to_owned(),
            ));
        }
        let payable = run
            .call("selecting a payable federation", || {
                Ok(self.inner.ports.payments.payable_federations(&policy_ids))
            })
            .await?
            .map_err(|error| FiError::Payment(error.to_string()))?
            .into_iter()
            .collect::<HashSet<_>>();
        Ok((policy_ids, payable))
    }

    async fn refresh_setup_payment_federations(
        &self,
        run: DriverRun<'_>,
    ) -> FiResult<Vec<AdmittedSetupPaymentFederation>> {
        let request_timeout = run.request_timeout();
        let publisher = self.inner.setup_payment_publisher.ok_or_else(|| {
            FiError::Payment(
                "paid formation requires a deployment-pinned setup-payment publisher".to_owned(),
            )
        })?;
        let current =
            self.inner
                .store
                .load_setup_payment_federations_event()
                .await
                .map(|event| {
                    restore_durably_admitted_setup_payment_federations_event(&event, publisher)
                        .map_err(|_| {
                            FiError::Storage(
                                "stored setup-payment publication failed authentication".to_owned(),
                            )
                        })
                })
                .transpose()?;

        let candidates = run
            .call("refreshing setup-payment policy", || {
                Ok(self
                    .inner
                    .ports
                    .registry
                    .fetch_setup_payment_federations(publisher, request_timeout))
            })
            .await?
            .unwrap_or_default();

        let now = Timestamp::from(fedimint_core::time::duration_since_epoch().as_secs());
        let mut admitted = current;
        for candidate in candidates
            .into_iter()
            .take(usize::from(SETUP_PAYMENT_FEDERATIONS_CANDIDATE_LIMIT))
        {
            if let Ok(candidate) =
                admit_setup_payment_federations_event(&candidate, publisher, now, admitted.as_ref())
            {
                admitted = Some(candidate);
            }
        }
        let admitted = admitted.ok_or_else(|| {
            FiError::Payment(
                "no authenticated setup-payment federation set is available".to_owned(),
            )
        })?;
        self.inner
            .store
            .store_setup_payment_federations_event(admitted.event().clone())
            .await?;

        Ok(admitted
            .set()
            .iter()
            .map(
                |(federation_id, invite_code)| AdmittedSetupPaymentFederation {
                    federation_id: federation_id.clone(),
                    invite_code: invite_code.clone(),
                },
            )
            .collect::<Vec<_>>())
    }
}
