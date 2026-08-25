# devmon

`devmon` (development monitor) is a read-only web dashboard for watching a running decentralized-federations environment. It subscribes to the Nostr relay(s) the components advertise on and streams every event to a browser, alongside a live FMan roster built from kind-37701 advertisements.

It is strictly an observer. It opens Nostr `REQ` subscriptions and serves HTTP, and it never publishes an event or calls a mutating RPC, so it can watch a live environment without perturbing it. It is a development tool: `publish = false`, not part of `selfci`, and nothing in the product depends on it.

## Quick start

One command brings up a relay, some FMan daemons advertising on it, and the dashboard, then holds it all open until you `Ctrl-C`:

```bash
cd crates/devmon           # from the repo root, inside `nix develop`
./run-env.sh
```

It prints the leased relay URL, each FMan's log path, and the dashboard address (`http://127.0.0.1:7777` by default). The relay is leased from `defe`, not hand-rolled: the script re-execs itself under `defe exec defe-cli --request-relay`, which starts a defe server, leases a Nostr relay, and hands its URL to the FMans and the dashboard. `run-env.sh` selects the Development Manifold profile, supplies that relay plus the test setup-payment publisher through its `MANIFOLD_DEV_*` override contract, and onboards each fresh disposable FMan as new through its local admin socket. The environment needs no `bitcoind`: a `fleet-manager` advertises without one and only starts its bundled `fedimintd` once a seat actually forms. That is enough to see real kind-37701 traffic, but it cannot run a formation. For that, use the `fleet_manager_0_1_formation` E2E.

Tunables, all optional environment variables:

- `FMAN_COUNT` sets the number of FMan daemons (default `3`).
- `DASH_PORT` sets the dashboard port (default `7777`).

For a busier demo: `FMAN_COUNT=5 ./run-env.sh`. Each FMan publishes once on
startup and then follows the production one-hour republish cadence.

## Pointing it at any relay

The relay is set from the page: type a URL into the header field and hit watch. This works against a relay `defe` leased for a test (its URL is logged), or a production relay, without a restart. Switching relays clears the view and starts fresh on the new one.

`--relay` sets the initial relay at startup and is optional; with none, the page just prompts for one:

```bash
cargo run -p devmon -- --relay ws://127.0.0.1:8880 --port 7777
```

## Viewing it over SSH

The dashboard binds `127.0.0.1` only. Forward the port with an explicit IPv4 target so `localhost` does not resolve to IPv6 `::1` on the remote host, which surfaces as `connect failed: Connection refused`:

```bash
ssh -L 7777:127.0.0.1:7777 <devbox>
```

Then open `http://localhost:7777` locally. The FMan roster refreshes on a short poll and is the reliable "is it alive" signal over a tunnel. The event feed rides a long-lived Server-Sent Events stream, which an intermediate HTTP proxy may buffer.

## Panels

- **FMan roster**, one card per FMan, derived entirely from its advertisements: whether it is accepting seats, offered plans, Iroh endpoint, embedded holder-authorization count, and whether the advertisement has expired. Because kind 37701 is addressable, each FMan has exactly one current advertisement, and a republish replaces the previous one.
- **Nostr event feed**, every event as it arrives, colour-coded and named by kind, filterable, click to expand the full signed document.

Further panels (formation timeline, seats and reservations, a component roster for `defe` and the push gateways, a merged log tail) are planned but not yet built.

## Security

Every field the dashboard renders comes off a public Nostr relay, which accepts signed events from anyone, so all of it is untrusted input. Values parsed from event content reach the page only through `textContent`, never `innerHTML`, so a hostile advertisement cannot inject markup. Keep it that way when extending the page.
