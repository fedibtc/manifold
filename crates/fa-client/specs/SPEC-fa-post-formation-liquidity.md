# SPEC-fa-post-formation-liquidity: FA to FLIP orchestration

## Status

MVP contract. The provider-side Public Liquidity API and the Manifold-owned FA
producer/recovery projection are implemented. Fedi owns launch supervision and
its RPC projection.

## Product boundary

FA and FLIP liquidity readers enforce a seven-day maximum age from each MG's
signed issuance time. They ignore the deprecated optional expiry hint and do
not negotiate lifetime through the request shape or an API capability.
Existing transfer and replacement policies remain separate.

During read-only MG acquisition, the FA retries only transport failures and
timeouts, using bounded per-call backoff followed by a separate capped operation
backoff. Without valid cached evidence, foreground reads have a five-minute
acquisition budget, bounded by the driver deadline. When valid evidence is due
for refresh or requires it, the FA makes one foreground connect/read attempt
bounded by 60 seconds, then revalidates the retained fallback. The worker retries
transient refresh failures until success or cancellation. The driver renews its
lease during collection. The FA does not retry service refusals, invalid evidence,
or credential admission.
These retries never enclose liquidity submission or change the exact request
commitment or status-before-replay rule.

FA stores one signed evidence document per exact MG identity, outside recovery
and backup authority. Formation, reopen/restore, and membership changes start
opportunistic acquisition without delaying formation success. While idle, the
worker sleeps until each document's refresh deadline and fetches when less than
eight hours remain in the seven-day reader window. Evidence with at least eight
hours remaining needs no network request except after a provider refusal or
for the liquidity-shopping compatibility refresh. After a failed refresh, the FA
retains and may use still-valid evidence, but never extends its age limit.

At the start of provider discovery, the FA makes one bounded MG read for each
cached document older than thirty minutes in a concurrent batch. This temporary
compatibility fetch helps old FLIPs that enforce the historical one-hour expiry;
maintainers may remove it when those readers no longer need support. Refresh
errors do not fail discovery or cause the FA to discard usable cached evidence.
The FA runs one batch per shopping entry without polling or automatic liquidity
resubmission, and keeps the seven-day reader policy. Provider discovery runs
alongside the batch with its own deadline. An old FLIP can still refuse retained
evidence if refreshing fails or shopping takes longer than the old expiry.

On every use, the FA rechecks identity, signature, bounds, and freshness. It
checks credentials and revocations live at admission. The FA invalidates only
the matching cached document when it definitively rejects all credential
candidates. Transient or indeterminate outcomes, or unchecked candidates, cause
it to reject admission but retain the document.

After an authenticated `invalid_credentials` or `policy_mismatch` rejection
from a provider, the FA marks only the submitted matching cached documents for
refresh on the next explicit start. It never resubmits liquidity automatically
or deletes usable fallback, and cannot mark a newer concurrent replacement.
The FA honors the marker on every cache shortcut; fetching the identical refused
document does not clear it. Cache writes retain a strictly newer valid document
transactionally and return that retained document to the caller.

Liquidity is post-formation work. `federation_created` remains the terminal
formation success state and is never delayed or rewritten by gateway or
stability-pool provisioning.

The Fedi formation flow may automatically request gateway/LN liquidity after a
federation is created. It does not request stability-pool liquidity. A
stability-only or combined request remains available through the same backend
for a later, administratively approved operation. MVP exposes no operation that
detaches a successfully attached Fedi-verified provider.

## Provider discovery and disclosure gate

Before sending an invite code, the FA performs a fresh bounded kind-37702
enumeration and locally admits provider advertisements. It verifies the Nostr
event signature and exact kind/d/hashtag, the event-author/provider identity,
the provider's canonical payload signature, issue/expiry bounds, supported
network and source, at least one live PeerBadge holder authorization under the
selected Manifold environment, and the signed Iroh endpoint/ALPN. Reaching the
Iroh endpoint must authenticate the node id carried by the signed URL.

Advertisements are a no-private-data preview. They are never cached across an
app exit/re-entry and a selected provider is refreshed immediately before the
private request. Any provider admitted by the selected environment's credential
verification is eligible; there is no consumer identity allowlist. When several
eligible providers pass the same product policy, selection is deterministic and
consumer-neutral. Resume re-admits only the provider named by the durable
commitment. A restored provider hint is never current trust.

## Request construction

Only a freshly reconciled formed record can request liquidity. From the final
invite and a real consensus read, the FA derives the authoritative federation
id, config hash, network, peers, and `fedi:fman_seat_bindings` directory. The FA
reconnects once to every distinct consensus-listed MG identity and fetches
`get_fman_trust_material` at request time. It verifies every response and
current PeerBadge, then carries:

- one peer attestation taken from the verified consensus directory plus its
  MG's matching live holder authorization as the bearer `fman_endorsement`;
  and
- one signed trust-material response for every distinct MG identity named by
  the consensus directory.

Consensus metadata, never requester-carried material, decides the identity and
seat set. Missing, duplicate, stale, contradictory, or unverifiable material
fails closed. There is no Nostr fallback. The carried `fleet_seat_hints` pair
each consensus seat binding with the persisted formation seat of the same MG
identity — never by list position — and a directory identity without a
matching persisted seat fails closed.

Gateway-only requests set `gateway_min_amount > 0`, optionally bound the
maximum, and set both stability amounts to zero/absent. Stability-only requests
do the inverse. A combined API/CLI request may set both. The FA computes the
canonical `details_payload_hash` with the shared service helper and signs each
Public Liquidity API payload with the FA key; it never duplicates canonical
serialization or domain separators.

## Durable lifecycle and idempotency

Before the first mutating RPC, persist the provider pubkey, signed Iroh endpoint,
exact request commitment fields, and `details_payload_hash`. MG endorsements
and trust material are refreshable proof and are not part of the commitment.
`start` gathers every MG proof before it persists the operation, so a request
that fails a local precondition leaves no operation behind to block the next
start.
At most one operation per federation may be non-terminal: `start` refuses with
a typed error naming the resumable operation while one exists, so a lost
response is resumed rather than shadowed by a second commitment.

Start and resume take the same run guard and durable driver lease as formation
runs: at most one formation or liquidity operation runs at a time, and a
concurrent call observes `Busy` rather than queueing. The consumer scheduler
must serialize its start/resume calls.

Both also share the same preconditions: the operation's formation must be the
formed federation, freshly reconciled, and resume additionally
requires a live consensus read still matching the persisted commitment
(network, invite code, federation id, config hash, and seat hints; the display
name is deliberately mutable). An accepted allocation is therefore not
recoverable through `resume` while the federation is unreachable, or after the
FA moves on to another formation or the seat directory changes — only the
read-only status and listing projections remain available.

After revalidating the active formed federation and exact persisted commitment
against fresh federation consensus, recovery order is:

1. if durable signed status already contains completed gateway evidence, replay
   its exact idempotent guardian fan-out and fresh readback before any provider
   discovery or connection;
2. if every requested item is durably completed and the gateway URL is verified
   when one was requested, return that durable result without requiring the
   provider to remain advertised or online;
3. otherwise refresh and re-verify the selected provider advertisement and
   endpoint, then query `GetAllocationStatus` by FA pubkey plus the persisted
   hash;
4. if found, adopt the provider-authoritative item states;
5. if not found and no durable acceptance evidence exists, and the commitment
   expired more than `FA_LIQUIDITY_EXPIRY_GRACE` ago by the local clock,
   durably mark the operation terminal with `request_expired`, without a
   replay;
6. otherwise, if not found and no durable acceptance evidence exists, refresh
   every MG proof and replay `RequestLiquidity` with the exact persisted
   commitment/hash — `not_found` against a durably persisted acceptance is a
   retained provider-consistency failure, never a replay or an expiry; and
7. persist the signed accepted/rejected response before projecting it.

After a signed gateway item becomes `completed`, FA validates that the evidence
variant, gateway id, fulfilled amount, and target amount agree. The evidence
also carries a canonical client-reachable `GatewayApiUrl`, derived by FLIP from
gatewayd's own registrations rather than its admin URL. FA fans the exact URL
out to every formed seat through signed `RegisterGateway` requests. It requires
the federation consensus threshold of successful acknowledgements (both a new
insert and an idempotent existing insert count), then requires a fresh upstream
LNv2 aggregate view to contain the exact URL. An unavailable or decommissioned
minority therefore does not block attachment.

The provider status is persisted before guardian registration. A separate
durable marker stores the exact canonical gateway URL only after threshold
acknowledgement and fresh readback. The write transaction rechecks that current
completion evidence still carries that URL; a later signed status carrying a
different URL clears the proof and requires a new fan-out/readback. The public
`gateway_view_verified` projection is true only when the marker and current
durable completion evidence match. Resume consumes that evidence before FLIP
rediscovery and repeats the idempotent fan-out while the exact URL lacks its
proof. This closes the crash interval between provider completion and client
discoverability. The exact-URL marker is an intentional pre-launch
liquidity-journal schema-3 reset rather than a silent interpretation of older
completed rows.

The federation id is FLIP's allocation identity and the hash is its semantic
retry identity. A lost response therefore never justifies inventing a new
hash. An exact request whose expiry passed can still recover an already accepted
allocation. FLIP refuses an expired request by its own clock, both before
verification and inside the transaction that would commit an allocation, so
`not_found` past expiry proves no allocation can ever exist for that hash. The
grace equals the provider-response freshness window and covers FLIP clock
skew. The terminal mark needs no MG proof, so an unreachable MG cannot keep an
expired operation live. As after a provider rejection, a new intent requires a
new explicit API invocation.

There is no FA-owned background task. The Fedi bridge owns durable task
scheduling and calls the library's bounded start/resume/status operations after
restart. To close the crash window between Manifold persistence and the original
`start` response, the bridge enumerates every durable operation through the
read-only, cursor-paginated `list_liquidity_operations` API and resumes each
non-terminal item by semantic id. Pages accept 1 through 100 rows, are ordered
by the canonical lowercase commitment hash, and fail closed if a stored key,
id, or recomputed commitment hash disagrees. Dropping one call is locally
cancellable and leaves the persisted checkpoint discoverable and recoverable.

## Progress and completion

Expose the provider, `details_payload_hash`, requested source bounds, the
durable gateway-view verification marker, and the provider-authoritative
per-item `pending`, `running`, `action_required`,
`completed`, `failed`, or `cancelled` states. There is deliberately no invented
overall provider state. Consumers may derive gateway readiness only when every
requested item is completed and `gateway_view_verified` is true.
Stability-only completion keeps its existing provider-evidence meaning.

Provider rejection is a terminal result for that exact intent but is not a
formation failure. `action_required` and the provider-side send-once
reconciliation states remain visible; the FA must not convert them into an
automatic retry that could duplicate an irreversible operation.

## Backup projection

An accepted request's compact FA backup must include provider pubkey, an
optional endpoint hint, and `details_payload_hash`. The hash is not derivable
from the master seed: its commitment binds the requester and provider pubkeys,
network, amount bounds, expiry, and the full `federation_details` — including
the invite code and federation name — so retaining the unresolved commitment
retains the invite code with it. Before provider acknowledgement, local durable
storage retains the exact commitment needed for safe replay; the encrypted
backup may additionally retain that unresolved commitment when seed-only
recovery of the in-flight request is required. Full trust material is never
backup authority and is refreshed.

Scope: this projection is a consumer (Fedi) obligation and is not yet
implemented anywhere in this repository — `fa-client` exposes only the durable
operations a backup would project, and the Nostr backup design
(`docs/fa-nostr-backups.md`) remains a draft.

## Security and resource bounds

- Do not disclose the invite until provider trust and endpoint identity pass.
- Bound relay candidates, advertisement bytes, embedded authorizations, MG
  fan-out, RPC request/response sizes, and every network deadline.
- Enforce freshness against the local clock: provider responses within ±1 hour
  of receipt, MG trust material no older than seven days, and advertisements no
  older than 4 hours regardless of their expiry. A skewed local clock rejects valid
  responses rather than accepting stale ones.
- Never log invite codes, trust documents, endpoint capabilities, or raw signed
  requests.
- Keep provider response verification independent of transport success.
- Accept only the shared canonical public HTTPS or identity-shaped Iroh
  gateway URL; never register the provider's admin URL.
- Preserve FLIP's documented send-once and `action_required` behavior. The
  upstream SP operation-id and gateway peg-in attribution gaps remain explicit;
  this orchestration layer cannot claim stronger completion than the provider.

## Alternatives considered

- **Use `setGatewayOverride`.** Rejected: it only selects a gateway that has
  already joined and cannot ask a provider to provision one.
- **Send the invite to a configured endpoint without advertisement checks.**
  Rejected: it bypasses the provider trust/disclosure gate and endpoint binding.
- **Resolve MG standing from Nostr inside FLIP.** Rejected by the settled
  trust-material-carriage decision; requester-carried, MG-authored material
  removes sequential relay lookups while consensus still names the operators.
- **Make liquidity part of `federation_created`.** Rejected: provider work is
  independently long-running and recoverable, and the product explicitly makes
  it post-formation.
- **Expose provider removal in MVP.** Rejected by product scope; successful
  Fedi-provider attachment is one-way in the app even though operators may add
  other gateways out of band.
