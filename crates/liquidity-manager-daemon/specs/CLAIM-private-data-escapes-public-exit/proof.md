# Current argument

## Argument

**L1 (`enum` + `code`) — public RPC and evidence exits select typed fields.**
The three listed handlers construct `GetProviderInfoResponse`,
`RequestLiquidityResponse`, and `GetAllocationStatusResponse`; none takes a
secret-store value. Status loading uses stored item target/status/evidence, and
the two listed completion-evidence variants have no classified field
([`public.rs`](../src/public.rs),
[`allocation_store.rs`](../src/allocation_store.rs),
[`service-liquidity-manager/src/public.rs`](../../service-liquidity-manager/src/public.rs)).

**L2 (`code`) — advertisement construction is an allowlist.** `republish`
constructs `LiquidityProviderAdvertisement` field by field from public setup and
the enrolled provider trust envelopes returned by
`holder_authorization::provider_trust_envelopes`, then `nostr` publishes its
canonical signed JSON; it does not serialize allocations, gateway credentials,
wallet observations, or invite-bearing targets
([`advertisement.rs`](../src/advertisement.rs),
[`holder_authorization.rs`](../src/holder_authorization.rs),
[`nostr.rs`](../src/nostr.rs)).

An enrolled envelope is a Holder-published document whose whole purpose is
public carriage, and it is the same `HolderAuthorizationEnvelope` shape the
advertisement has always embedded, so moving its source does not widen this
exit. The classified "request-carried FMan trust material" is a distinct
object reaching a distinct surface and is unaffected.

**L3 (`enum` + `code`) — health and the startup-argument log avoid classified
values.** Normal/restore unauthenticated health constructs only health
component/status data. The startup `?args` tracing invocation uses
`DaemonArgs::Debug`, which replaces token,
secret-store-key, and provider private key with `<redacted>`
([`daemon.rs`](../src/daemon.rs), [`admin.rs`](../src/admin.rs)).

**L4 (`code`) — the target peg-in address reaches logs.**
`stability_allocation` emits the target `peg_in_address` in a tracing event. That
value is a classified private federation datum in this claim and the log is an
enumerated public exit, so the claim is false
([`stability_allocation.rs`](../src/stability_allocation.rs)).

## Residual windows

- Authenticated Admin views and error bodies are outside this enumerated
  public-exit claim; `SPEC-flip-admin-api` designates that surface private.
- Unencrypted backup archives and a local filesystem reader are outside the
  observer model; `SPEC-flip-admin-api` expressly identifies archives as secret
  material.

## Weakest links

1. **L4 (`code`)** — target address tracing.
2. **L1 (`enum`/`code`)** — all public response and evidence writers.
3. **L2–L3 (`code`)** — advertisement field selection and logging call sites.
4. **A1–A2 (`axiom`)** — serializer and log-observer boundaries.
