//! Authenticated common-set refresh, durable high-water retention, and selection.

use std::collections::HashSet;

use fedi_decentralized_domain::DEFAULT_SETUP_PAYMENT_MIN_FEE_PPM;
use fedi_decentralized_nostr::setup_payment_federations::{
    admit_setup_payment_federations_event, restore_durably_admitted_setup_payment_federations_event,
};
use fedi_decentralized_nostr_clients::{FaNostrClient, SETUP_PAYMENT_FEDERATIONS_CANDIDATE_LIMIT};
use fedi_decentralized_service_manifold_guardian::{FederationId, InviteCode};
use nostr_sdk::Timestamp;

use crate::formation::DriverRun;
use crate::{
    FaClient, FaError, FaIdentity, FaPayments, FaResult, FederationConsensusReader,
    ManifoldGuardianConnector,
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

impl<I, P, N, F, C> FaClient<I, P, N, F, C>
where
    I: FaIdentity,
    P: FaPayments,
    N: FaNostrClient,
    F: ManifoldGuardianConnector,
    C: FederationConsensusReader,
{
    /// Return the authenticated setup-payment policy set in canonical order,
    /// each member paired with the signed invite that joins it.
    ///
    /// This includes admitted federations whose joined wallet currently has
    /// zero balance. Consumers can intersect the members with their own joined
    /// wallet projection to render payer/refill choices; Pay-and-create still
    /// validates the selected payer through [`FaPayments::payable_federations`]
    /// and the wallet's funding check for each seat.
    ///
    /// A consumer needs the invite to offer a member it has **not** joined: an
    /// id cannot be turned back into an invite.
    pub async fn admitted_setup_payment_federations(
        &self,
        options: crate::FormationRunOptions,
    ) -> FaResult<Vec<AdmittedSetupPaymentFederation>> {
        let _run = self.claim_run_guard().await?;
        options.validate_for_start()?;
        let run = DriverRun::start(options)?;
        self.refresh_setup_payment_federations(run).await
    }

    /// The smallest guardian fee rate this FA may propose right now, in ppm.
    ///
    /// Reads the durably admitted publication rather than refreshing it: the
    /// floor gates a local intent before any guardian is contacted, so it has
    /// to hold with the relays unreachable. Fails safe upwards exactly like
    /// the MG-side resolution — a deployment with no pinned publisher, no
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

    /// Validate one consumer-selected payer against authenticated policy and
    /// current wallet readiness without changing the caller's choice.
    pub(crate) async fn require_setup_payment_federation(
        &self,
        requested: &FederationId,
        run: DriverRun,
    ) -> FaResult<FederationId> {
        let (policy_ids, payable) =
            self.payable_setup_payment_federations(run)
                .await
                .map_err(|error| match error {
                    FaError::Payment(_) => FaError::SelectionReauthorizationRequired {
                        reason: crate::SelectionReauthorizationReason::SelectedPayerUnavailable,
                        failure: None,
                    },
                    error => error,
                })?;
        if !policy_ids.contains(requested) || !payable.contains(requested) {
            return Err(FaError::SelectionReauthorizationRequired {
                reason: crate::SelectionReauthorizationReason::SelectedPayerUnavailable,
                failure: None,
            });
        }
        Ok(requested.clone())
    }

    pub(crate) async fn payable_setup_payment_federations(
        &self,
        run: DriverRun,
    ) -> FaResult<(Vec<FederationId>, HashSet<FederationId>)> {
        let policy_ids = self
            .refresh_setup_payment_federations(run)
            .await?
            .into_iter()
            .map(|member| member.federation_id)
            .collect::<Vec<_>>();
        if policy_ids.is_empty() {
            return Err(FaError::Payment(
                "the authenticated setup-payment federation set is empty".to_owned(),
            ));
        }
        let payable = run
            .call("selecting a payable federation", || {
                Ok(self.inner.ports.payments.payable_federations(&policy_ids))
            })
            .await?
            .map_err(|error| FaError::Payment(error.to_string()))?
            .into_iter()
            .collect::<HashSet<_>>();
        Ok((policy_ids, payable))
    }

    async fn refresh_setup_payment_federations(
        &self,
        run: DriverRun,
    ) -> FaResult<Vec<AdmittedSetupPaymentFederation>> {
        let request_timeout = run.request_timeout();
        let publisher = self.inner.setup_payment_publisher.ok_or_else(|| {
            FaError::Payment(
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
                            FaError::Storage(
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
            FaError::Payment(
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
