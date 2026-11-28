//! Which hosts an unbadged collector contacts for a registered endpoint id.
//!
//! An unbadged registrant chooses its endpoint id freely, and the owner of an
//! endpoint id publishes its own relay URL and direct addresses. Dialing what
//! it publishes would let any key make the collector send traffic to hosts
//! of its choosing, including the collector's private network. The unbadged
//! collector therefore has no direct IP transport and reaches every MG only
//! through the relays it was built with: a lookup result keeps only those
//! relay URLs, so the published data selects among them and nothing else.

use std::sync::Arc;

use fedi_iroh_rpc::iroh::{
    Endpoint, EndpointId, RelayMap, TransportAddr,
    address_lookup::{
        AddrFilter, AddressLookup, AddressLookupBuilder, AddressLookupBuilderError,
        DnsAddressLookup, EndpointInfo, Error, Item, PkarrResolver,
    },
    endpoint::{BindError, default_relay_mode, presets},
};
use futures_util::StreamExt as _;

/// Bind the unbadged collector's endpoint: relay-only, through its own relays.
pub(crate) async fn bind_relay_only() -> Result<Endpoint, BindError> {
    let relay_mode = default_relay_mode();
    let relays = relay_mode.relay_map();
    Endpoint::builder(presets::Minimal)
        .clear_ip_transports()
        .relay_mode(relay_mode)
        .address_lookup(OwnRelaysOnlyBuilder::new(
            PkarrResolver::n0_dns(),
            relays.clone(),
        ))
        .address_lookup(OwnRelaysOnlyBuilder::new(
            DnsAddressLookup::n0_dns(),
            relays,
        ))
        .bind()
        .await
}

/// Builds an [`OwnRelaysOnly`] around another lookup's builder.
#[derive(Debug)]
struct OwnRelaysOnlyBuilder<B> {
    inner: B,
    filter: AddrFilter,
}

/// Address lookup that drops every address except the given relays.
#[derive(Debug)]
struct OwnRelaysOnly<L> {
    inner: L,
    filter: AddrFilter,
}

impl<B> OwnRelaysOnlyBuilder<B> {
    fn new(inner: B, relays: RelayMap) -> Self {
        Self {
            inner,
            filter: own_relays_only(relays),
        }
    }
}

fn own_relays_only(relays: RelayMap) -> AddrFilter {
    let relays = Arc::new(relays);
    AddrFilter::new(move |addrs| {
        std::borrow::Cow::Owned(
            addrs
                .iter()
                .filter(|addr| match addr {
                    TransportAddr::Relay(url) => relays.contains(url),
                    _ => false,
                })
                .cloned()
                .collect(),
        )
    })
}

impl<B: AddressLookupBuilder> AddressLookupBuilder for OwnRelaysOnlyBuilder<B> {
    fn into_address_lookup(
        self,
        endpoint: &Endpoint,
    ) -> Result<impl AddressLookup, AddressLookupBuilderError> {
        Ok(OwnRelaysOnly {
            inner: self.inner.into_address_lookup(endpoint)?,
            filter: self.filter,
        })
    }
}

impl<T: AddressLookup> AddressLookup for OwnRelaysOnly<T> {
    fn resolve(
        &self,
        endpoint_id: EndpointId,
    ) -> Option<futures_util::stream::BoxStream<'static, Result<Item, Error>>> {
        let filter = self.filter.clone();
        Some(
            self.inner
                .resolve(endpoint_id)?
                .map(move |item| item.map(|item| keep_allowed(&filter, &item)))
                .boxed(),
        )
    }
}

fn keep_allowed(filter: &AddrFilter, item: &Item) -> Item {
    Item::new(
        EndpointInfo::from_parts(
            item.endpoint_id(),
            item.endpoint_info().data.apply_filter(filter).into_owned(),
        ),
        item.provenance(),
        item.last_updated(),
    )
}

#[cfg(test)]
mod tests {
    use fedi_iroh_rpc::iroh::{RelayUrl, SecretKey, address_lookup::EndpointData};

    use super::*;

    /// A lookup that answers with what an endpoint's owner chose to publish.
    #[derive(Debug)]
    struct Published(Vec<TransportAddr>);

    impl AddressLookup for Published {
        fn resolve(
            &self,
            endpoint_id: EndpointId,
        ) -> Option<futures_util::stream::BoxStream<'static, Result<Item, Error>>> {
            let item = Item::new(
                EndpointInfo::from_parts(endpoint_id, EndpointData::new(self.0.clone())),
                "published",
                Some(7),
            );
            Some(futures_util::stream::iter([Ok(item)]).boxed())
        }
    }

    #[tokio::test]
    async fn a_lookup_keeps_only_the_collectors_own_relays() {
        let own: RelayUrl = "https://own-relay.example./".parse().unwrap();
        let chosen: RelayUrl = "https://10.0.0.5/".parse().unwrap();
        let lookup = OwnRelaysOnly {
            inner: Published(vec![
                TransportAddr::Relay(chosen),
                TransportAddr::Ip("10.0.0.5:4433".parse().unwrap()),
                TransportAddr::Ip("203.0.113.9:4433".parse().unwrap()),
                TransportAddr::Relay(own.clone()),
            ]),
            filter: own_relays_only(RelayMap::from_iter([own.clone()])),
        };
        let endpoint_id = SecretKey::from_bytes(&[3; 32]).public();
        let items: Vec<_> = lookup.resolve(endpoint_id).unwrap().collect().await;
        let [Ok(item)] = items.as_slice() else {
            panic!("one item expected: {items:?}");
        };
        assert_eq!(item.endpoint_id(), endpoint_id);
        assert_eq!(item.provenance(), "published");
        assert_eq!(item.last_updated(), Some(7));
        assert_eq!(
            item.addrs().cloned().collect::<Vec<_>>(),
            [TransportAddr::Relay(own)]
        );
    }

    #[tokio::test]
    async fn the_unbadged_endpoint_has_no_direct_transport() {
        let endpoint = bind_relay_only().await.unwrap();
        assert!(endpoint.bound_sockets().is_empty());
        endpoint.close().await;
    }
}
