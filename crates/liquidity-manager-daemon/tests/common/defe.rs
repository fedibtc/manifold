use anyhow::Context;
use defe_client::{
    AsyncDefeClient, BitcoindInfo, NostrRelayInfo, ResourceDescriptor, ResourceHandleId,
    SharingMode,
};

pub struct DefeResources {
    client: AsyncDefeClient,
    relay: Option<(ResourceHandleId, NostrRelayInfo)>,
    bitcoind: Option<(ResourceHandleId, BitcoindInfo)>,
}

impl DefeResources {
    pub async fn relay_only() -> anyhow::Result<Self> {
        let mut client = AsyncDefeClient::connect_from_env()
            .await
            .context("connect to defe from env")?;
        let relay = allocate_relay(&mut client).await?;
        Ok(Self {
            client,
            relay: Some(relay),
            bitcoind: None,
        })
    }

    pub async fn live_liquidity() -> anyhow::Result<Self> {
        let mut client = AsyncDefeClient::connect_from_env()
            .await
            .context("connect to defe from env")?;
        let relay = allocate_relay(&mut client).await?;
        let bitcoind = allocate_bitcoind(&mut client).await?;
        Ok(Self {
            client,
            relay: Some(relay),
            bitcoind: Some(bitcoind),
        })
    }

    pub fn relay(&self) -> &NostrRelayInfo {
        &self.relay.as_ref().expect("relay resource was allocated").1
    }

    pub fn bitcoind(&self) -> &BitcoindInfo {
        &self
            .bitcoind
            .as_ref()
            .expect("bitcoind resource was allocated")
            .1
    }

    pub async fn release(mut self) -> anyhow::Result<()> {
        if let Some((handle, _)) = self.bitcoind.take() {
            self.client
                .release(handle)
                .await
                .context("release defe bitcoind")?;
        }
        if let Some((handle, _)) = self.relay.take() {
            self.client
                .release(handle)
                .await
                .context("release defe Nostr relay")?;
        }
        Ok(())
    }
}

async fn allocate_relay(
    client: &mut AsyncDefeClient,
) -> anyhow::Result<(ResourceHandleId, NostrRelayInfo)> {
    let lease = client
        .request_nostr_relay(SharingMode::Exclusive)
        .await
        .context("allocate exclusive defe Nostr relay")?;
    let ResourceDescriptor::NostrRelay(info) = lease.descriptor else {
        unreachable!("request_nostr_relay validates its descriptor")
    };
    Ok((lease.handle_id, info))
}

async fn allocate_bitcoind(
    client: &mut AsyncDefeClient,
) -> anyhow::Result<(ResourceHandleId, BitcoindInfo)> {
    let lease = client
        .request_bitcoind(SharingMode::Exclusive)
        .await
        .context("allocate exclusive defe bitcoind")?;
    let ResourceDescriptor::Bitcoind(info) = lease.descriptor else {
        unreachable!("request_bitcoind validates its descriptor")
    };
    Ok((lease.handle_id, info))
}
