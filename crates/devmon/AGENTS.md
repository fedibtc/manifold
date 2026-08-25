# devmon notes

- Development tool only: `publish = false`, kept out of `selfci`, nothing in the product depends on it. Do not let product code depend on it.
- Read-only observer. It opens Nostr `REQ` subscriptions and serves HTTP. Never add a publish, a mutating RPC, or any write to a watched component. If it cannot reach a data source, degrade that panel and keep serving.
- Everything rendered comes off a public relay and is untrusted input. Relay-derived values reach the page only via `textContent`, never `innerHTML`. Keep new panels the same way.
- The crate depends only on `crates/nostr` (for the kind constants), so it compiles on master. Keep it that way: do not depend on `fleet-manager` or other daemon crates just to reuse a type. Advertisements are parsed structurally in `parse_advertisement` precisely to avoid that dependency.
- `run-env.sh` launches the `fleet-manager` binary from the `fman` package. It
  selects the Development profile and routes its leased relay and test publisher
  through `MANIFOLD_DEV_NOSTR_RELAYS` and
  `MANIFOLD_DEV_SETUP_PAYMENT_PUBLISHER`; never restore per-binary trust-routing
  flags. Fresh data roots must be onboarded through the local admin socket
  before waiting for advertisements.
- Keep `./README.md` synchronized when changing panels, CLI flags, `run-env.sh` behavior, or the set of decoded kinds.
