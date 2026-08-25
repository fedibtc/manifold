use std::collections::BTreeMap;

use fedi_decentralized_domain::GatewayApiUrl;
use fedimint_core::PeerId;
use fedimint_core::util::SafeUrl;

use super::admitted_lnv2_gateway_view;

#[test]
fn gateway_view_keeps_valid_entries_when_unrelated_entries_are_outside_policy() {
    let gateway = SafeUrl::parse("https://gateway.example/").expect("valid gateway URL");
    let legacy = SafeUrl::parse("http://legacy.example/").expect("valid legacy SafeUrl");
    let responses = BTreeMap::from([
        (PeerId::from(0), vec![gateway.clone(), legacy]),
        (PeerId::from(1), vec![gateway]),
    ]);

    assert_eq!(
        admitted_lnv2_gateway_view(&responses),
        vec![GatewayApiUrl::try_from("https://gateway.example/").expect("admitted gateway")],
    );
}
