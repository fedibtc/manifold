//! Bind a pairing-only endpoint on explicit request. Its random endpoint key
//! is not the fleet's long-lived Iroh identity; early setup must neither
//! advertise that identity nor bypass the duplicate-host startup guard.

use fedi_decentralized_service_manifold_guardian::holder_pairing::{
    HOLDER_PAIRING_ALPN, HolderPairingInvite, HolderPairingServiceServer,
};
use fedi_iroh_rpc::IrohProtocol;
use iroh::Endpoint;
use iroh::endpoint::presets;
use iroh::protocol::Router;
use mg_nostr::holder_pairing::HolderPairing;
use tokio::sync::Mutex;

pub(crate) struct PairingEndpoint {
    pairing: HolderPairing,
    state: Mutex<State>,
}

#[derive(Default)]
enum State {
    #[default]
    Idle,
    Running(Router),
    Fenced,
}

impl PairingEndpoint {
    pub(crate) fn new(pairing: HolderPairing) -> Self {
        Self {
            pairing,
            state: Mutex::default(),
        }
    }
    /// Prevent even already accepted admin requests from minting new grants.
    pub(crate) async fn fence(&self, db: &mg_core::db::Db) -> anyhow::Result<()> {
        let old = std::mem::replace(&mut *self.state.lock().await, State::Fenced);
        db.clear_holder_grant().await?;
        if let State::Running(router) = old {
            router.shutdown().await?;
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl mg_core::admin::HolderPairingOffers for PairingEndpoint {
    async fn create(&self) -> anyhow::Result<HolderPairingInvite> {
        let mut state = self.state.lock().await;
        let local = std::env::var_os("MG_E2E_LOCAL_IROH").is_some();
        if matches!(*state, State::Idle) {
            let builder = if local {
                Endpoint::builder(presets::N0DisableRelay)
                    .bind_addr(std::net::SocketAddr::from(([127, 0, 0, 1], 0)))?
            } else {
                Endpoint::builder(presets::N0)
            };
            let endpoint = builder.bind().await?;
            *state = State::Running(
                Router::builder(endpoint)
                    .accept(
                        HOLDER_PAIRING_ALPN,
                        IrohProtocol::with_limits_and_request_read_timeout(
                            HolderPairingServiceServer::new(self.pairing.clone()),
                            96 * 1024,
                            4,
                            std::time::Duration::from_secs(5),
                        ),
                    )
                    .spawn(),
            );
        }
        let State::Running(router) = &*state else {
            anyhow::bail!("holder pairing disabled");
        };
        let mut invite = self
            .pairing
            .offer(router.endpoint().id().to_string())
            .await?;
        if local {
            invite.direct_address = router
                .endpoint()
                .bound_sockets()
                .into_iter()
                .find(|a| a.ip().is_loopback());
            anyhow::ensure!(
                invite.direct_address.is_some(),
                "missing local pairing route"
            );
        }
        Ok(invite)
    }
}
