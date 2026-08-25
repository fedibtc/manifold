# REQ-no-public-ip: No public IP, no DNS, no TLS

Source: load-bearing product constraint. If a future requirement cannot be
met under this constraint, the FMan design is wrong, not the constraint.

Every network surface the FMan exposes or consumes must work on a host
with no inbound port reachability and no operator-procured certificate.
Operators run FMans on home hardware, Start9/Umbrel boxes, and cheap VPSs;
requiring a public endpoint or a certificate would exclude exactly the
operators the system is for.

How each surface satisfies it today:

- FI ↔ FMan control plane: iroh (relay/hole-punched, NodeId-addressed),
  framed by `fedi-iroh-rpc`. No listener port.
- Fedi collector ↔ guardian Prometheus: a dedicated, seat-capability-scoped
  Iroh protocol at the FMan proxies the child's loopback response. No public
  HTTP listener ([SPEC-guardian-telemetry-proxy](./SPEC-guardian-telemetry-proxy.md)).
- FMan → registry: outbound Nostr publishes to operator-chosen relays.
- fedimintd peer and client traffic: fedimintd's own iroh connector
  (`--enable-iroh`), per-seat deterministic NodeIds.
- FMan wallet → payment federations: fedimint-client, outbound only.
- FMan → push gateway: outbound-only HTTPS to one deployment-pinned origin for
  non-load-bearing DKG completion callbacks. Development may explicitly use
  HTTP only on a numeric loopback origin
  ([SPEC-fi-rpc](./SPEC-fi-rpc.md), *DKG completion callback*).
- Operator → admin: a local Unix socket, plus an optional platform-routed UI.
  The UI requires no operator-procured address or certificate and is either
  isolated behind an authenticated platform proxy or password-authenticated
  when the platform exposes its LAN/Tor route
  ([SPEC-admin-socket](./SPEC-admin-socket.md),
  [SPEC-operator-http](./SPEC-operator-http.md)).
- fedimintd ui/metrics ports: bound to `127.0.0.1` only. The p2p/api ports
  bind all interfaces because fedimintd places its iroh UDP sockets there and
  a loopback bind would forfeit hole-punched direct paths (relay-only). The
  surface still requires no inbound reachability — relays remain the
  fallback. In iroh mode there is no TCP listener at the p2p address; the
  api address also carries fedimintd's plaintext WebSocket client API, the
  same designed-public API already served to every iroh dialer (admin verbs
  gated by the seat's `api_auth`), so no new confidential surface appears.
  That TCP listener is an incidental co-located exposure: fedimintd takes one
  address per port and offers no way to bind the iroh UDP socket and the TCP
  listener separately.

Acceptance condition for any new surface: it must appear in this list
with an outbound-only, localhost-only, or NAT-traversing mechanism.
