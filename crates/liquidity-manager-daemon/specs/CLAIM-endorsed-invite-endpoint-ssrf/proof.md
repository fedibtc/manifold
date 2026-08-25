# Current argument

## Argument

**L1 (`code`) — the endorsement authenticates only the federation id before
preview.** `run_admission_gate` parses the federation id from the FI's invite,
checks it against the FMan attestation, and verifies the endorsement badge. It
neither compares an API URL to an attested value nor obtains a trusted endpoint
directory. The FMan attestation itself binds federation/config/seat identity,
not an invite API URL.

**L2 (`code`) — a caller can alter the endpoint while retaining that id.** The
pinned `InviteCode::new` accepts an arbitrary `SafeUrl`, peer id, and supplied
`FederationId` as independent fields. `SafeUrl::parse` delegates directly to
URL parsing and applies no loopback/private-address policy. Thus an FI can
construct a valid invite whose federation-id part passes L1 while its API part
is an attacker-selected globally routed `wss://` endpoint.

**L3 (`code` + concrete execution) — FLIP dials the altered endpoint before
config authentication.** After L1, `VerificationPipeline::run_pipeline` calls
`FederationPreviewProvider::preview`. The production provider calls shared
`federation_preview::preview`, whose `read_consensus` invokes
`download_from_invite_code`. That dependency builds `DynGlobalApi` from
`invite.peers()` and requests the client config; `DynGlobalApi` routes that
request through `ConnectorRegistry::connect_guardian`, whose default WebSocket
connector gives `url.as_str()` directly to `WsClientBuilder::build`. The client
defaults have a small fixed exact-URL override map, so the witness chooses an
unmapped attacker-controlled global endpoint; it reaches the enabled
WebSocket connector unchanged. Only the response is later checked to calculate
the same federation id. Therefore the request's altered API URL is used for a
connection attempt before the configuration's identity, seat bindings, FMan
advertisements, or policy are authenticated.

The allocation fast path does not prevent the witness: it uses an
unallocated endorsed federation. Alternatively, the FI can set the
FI-supplied `federation_details.federation_id` to a fresh value; that field is
not checked against the invite until after preview has already dialed. By
A1–A2, this makes one valid endorsement an SSRF/network-probing capability and
falsifies the claim.

## Residual windows

- A malformed invite, an unready provider, invalid RPC signature/hash, or an
  unavailable fresh revocation lookup fails before preview. L3 assumes their
  normal successful preconditions, uses a syntactically valid unmapped
  endpoint, and needs only an attempted connection.
- The final config-id comparison prevents a fake endpoint from normally
  producing an accepted allocation. It is not an outbound-connect guard.
- Production applies `GlobalOnly` at every invite-derived dial, including
  WebSocket re-resolution and redirects. This rejects loopback, link-local,
  transition-address, and other non-global endpoints. Preview failures expose
  only sanitized detail to the requester. These controls limit reachability and
  returned information but do not authenticate a globally routed invite URL
  before the initial dial.
- This is separate from replay amplification: a single request establishes the
  unsafe network capability; replay only increases its volume.

## Weakest links

1. **L3 (`code`)** — the shared preview transport must be regenerated when
   invite discovery or connector routing changes.
2. **L1–L2 (`code`)** — gate ordering and invite URL semantics are local/pinned
   dependency facts.
3. **A1–A2 (`axiom`)** — deployment network topology gives the SSRF effect its
   security consequence.
