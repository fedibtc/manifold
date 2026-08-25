//! Invite-code federation preview boundary.
//!
//! The verifier derives the authoritative federation identity, config hash,
//! peer set, network, and consensus threshold from the invite code, plus the
//! raw `fedi:fman_seat_bindings` consensus metadata value.
//!
//! The transport and the derivation live in
//! [`fedi_decentralized_federation_preview`], shared with the FI's post-DKG
//! readback so both components read consensus the same way. What stays here is
//! FLIP's own boundary: the provider trait it injects, and the fixture and
//! fake implementations its tests and non-mainnet deployments wire in.

use async_trait::async_trait;
use fedi_decentralized_service_liquidity_manager::InviteCode;
use fedimint_connectors::ConnectorRegistry;

use crate::endpoint_policy::{self, EndpointPolicy, EndpointPolicyError};

// `unreachable_pub` does not follow the re-export path: this module is private,
// but `lib.rs` forwards these two types to the crate root, where the live
// liquidity harness imports them. Narrowing this is an E0365.
#[allow(unreachable_pub)]
pub use fedi_decentralized_federation_preview::{
    FederationPreview, FederationPreviewError, PreviewPeer,
};

fn map_endpoint_policy_error(error: EndpointPolicyError) -> FederationPreviewError {
    match error {
        EndpointPolicyError::MalformedInvite => {
            FederationPreviewError::InvalidInviteCode("invite code is malformed".to_owned())
        }
        EndpointPolicyError::UnsupportedScheme
        | EndpointPolicyError::InvalidIrohEndpoint
        | EndpointPolicyError::TooManyEndpoints
        | EndpointPolicyError::WebSocketDisallowed => {
            FederationPreviewError::EndpointPolicyRejected
        }
    }
}

/// Boundary for previewing a federation from its invite code.
#[async_trait]
pub(crate) trait FederationPreviewProvider: Send + Sync {
    /// Preview the federation identified by `invite_code`.
    async fn preview(
        &self,
        invite_code: &InviteCode,
    ) -> Result<FederationPreview, FederationPreviewError>;
}

/// Invite-code preview over the real Fedimint client API.
pub(crate) struct FedimintFederationPreviewProvider {
    connectors: ConnectorRegistry,
    endpoint_policy: EndpointPolicy,
}

impl FedimintFederationPreviewProvider {
    /// Bind the client connector registry this provider dials through, under
    /// `endpoint_policy`.
    ///
    /// # Errors
    ///
    /// Returns an error if the connector registry cannot be bound.
    pub(crate) async fn new(endpoint_policy: EndpointPolicy) -> anyhow::Result<Self> {
        Ok(Self {
            connectors: fedi_decentralized_federation_preview::bind_client_connectors().await?,
            endpoint_policy,
        })
    }
}

#[async_trait]
impl FederationPreviewProvider for FedimintFederationPreviewProvider {
    async fn preview(
        &self,
        invite_code: &InviteCode,
    ) -> Result<FederationPreview, FederationPreviewError> {
        // The guard belongs here, at the only implementation that dials.
        // Putting it in the pipeline instead would place it where the fixture
        // and fake providers also run, testing the check against
        // implementations that never open a socket while leaving this one able
        // to drift away from it.
        let pinned_invite =
            endpoint_policy::check_invite_endpoints(self.endpoint_policy, &invite_code.0)
                .await
                .map_err(map_endpoint_policy_error)?;
        // The federation id, never the invite code: an invite is private
        // federation detail that travels only over the public RPC, and a log
        // file is not that RPC.
        //
        // Debug on both arms, because a requester chooses how often this runs
        // and the refusal it produces is already reported once at the public
        // boundary.
        let federation_id = pinned_invite.federation_id();
        tracing::debug!(%federation_id, "previewing a target federation over its own endpoints");
        let preview = fedi_decentralized_federation_preview::preview(
            &self.connectors,
            &InviteCode(pinned_invite.to_string()),
        )
        .await;
        match &preview {
            Ok(_) => tracing::debug!(%federation_id, "federation preview answered"),
            Err(error) => tracing::debug!(%federation_id, %error, "federation preview failed"),
        }
        preview
    }
}

#[cfg(test)]
#[path = "../tests/federation_preview.rs"]
mod tests;

#[cfg(test)]
pub(crate) mod test_fakes {
    use std::collections::HashMap;
    use std::sync::Mutex;

    use super::*;

    /// Programmable fake preview provider keyed by invite code.
    #[derive(Default)]
    pub(crate) struct FakeFederationPreviewProvider {
        previews: Mutex<HashMap<String, Result<FederationPreview, String>>>,
    }

    impl FakeFederationPreviewProvider {
        pub(crate) fn respond_ok(&self, invite_code: &str, preview: FederationPreview) {
            self.previews
                .lock()
                .expect("fake preview lock")
                .insert(invite_code.to_owned(), Ok(preview));
        }

        pub(crate) fn respond_invalid(&self, invite_code: &str, reason: &str) {
            self.previews
                .lock()
                .expect("fake preview lock")
                .insert(invite_code.to_owned(), Err(reason.to_owned()));
        }
    }

    #[async_trait]
    impl FederationPreviewProvider for FakeFederationPreviewProvider {
        async fn preview(
            &self,
            invite_code: &InviteCode,
        ) -> Result<FederationPreview, FederationPreviewError> {
            match self
                .previews
                .lock()
                .expect("fake preview lock")
                .get(&invite_code.0)
            {
                Some(Ok(preview)) => Ok(preview.clone()),
                Some(Err(reason)) => Err(FederationPreviewError::InvalidInviteCode(reason.clone())),
                None => Err(FederationPreviewError::Unavailable(
                    "no fake preview programmed for this invite code".to_owned(),
                )),
            }
        }
    }
}
