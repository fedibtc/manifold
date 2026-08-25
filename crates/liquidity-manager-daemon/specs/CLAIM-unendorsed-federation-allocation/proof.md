# Current argument

## Argument

**L1 (`enum` + `code`) — the official daemon installs the real pipeline and the
sole public allocation creator calls it before writing.** `run_daemon`, the
normal-mode binary construction path, always installs `VerificationPipeline`
with the Nostr revocation fetcher and the selected profile's validated
`PeerBadgeTrustPolicy`; fixtures can substitute preview data but not the
pipeline. Regenerating production writers of `allocations` and
`allocation_items` then yields only `allocation_store::insert_allocation`. Its
sole production call site is `accept_or_reject_request`. The outer
`DaemonContext::request_liquidity` boundary first verifies the signed request
and rejects a wrong provider key before calling it. `accept_or_reject_request`
then returns an existing allocation without creating work, runs pre-validation,
and calls the verification provider; any rejection returns a stateless signed
rejection. Only a non-rejected outcome reaches the SQLite write transaction and
`insert_allocation`
([`daemon.rs`](../src/daemon.rs), [`public.rs`](../src/public.rs)).
`StaticVerificationProvider` is an explicit test-support double, not a
`run_daemon` runtime mode.

**L2 (`code` + `test`) — verification cannot pass without the exact admission
capability.** `VerificationPipeline::run_pipeline` calls `run_admission_gate`
before preview or any later trust stage. The gate requires the optional
endorsement, verifies its attestation, parses the federation id from the invite
code and compares it to the statement, verifies the holder envelope against
installed authorities with the attesting FMan as subject, requires the
authentic badge to meet the profile minimum, then reruns that verification with
a fresh clock sample after a successful revocation lookup.
Every failure returns a rejection, so it cannot reach L1's writer. The unit
matrix covers absent,
unverifiable, wrong-federation, untrusted-issuer, wrong-FMan, below-minimum, and
revoked endorsements, including the gate-before-preview ordering
([`verification.rs`](../src/verification.rs)). A1 and A2 give these predicates
their external meaning.

**L3 (`code`) — the committed row is for the federation the capability admitted.**
After L2, the pipeline previews the invite and rejects unless the request's
claimed federation id, config hash, and network match that preview. The
allocation writer derives its `FundingTargetRecord` and primary key from that
now-verified request federation id. It also rejects unless the preview's
federation id equals the id parsed in the admission gate, so an endpoint or
fixture cannot substitute another federation between the two checks. The
focused mismatched-preview test pins that join. Thus an accepted endorsement
for one invite federation cannot create an allocation for a different request
federation ([`verification.rs`](../src/verification.rs),
[`allocation_store.rs`](../src/allocation_store.rs)).

**L4 (`schema` + `code`) — crashes and racing deliveries cannot create an
unverified row.** The parent row and its items are inserted in L1's one SQLite
transaction. A crash before commit leaves neither durable; a crash after commit
has already passed L2. A racing winner which has passed its own L2 is the only
one that can commit; a loser rolls back and reads the existing allocation. A3
therefore preserves the claim at every durable state.

## Residual windows

- A valid endorsement is transferable and has no expiry; a holder may use it
  even if it is not the FI or current transport actor. `SPEC-flip-rpc` defines
  this bearer-authorization boundary; this claim provides no requester-identity
  guarantee.
- An allocation accepted before a later credential revocation remains durable
  and its workers may continue. The claim is about the admission point; FLIP
  has no continuous authorization rule.
- A credential can be revoked after the lookup responds and before the SQLite
  commit. This record asserts the authoritative observations FLIP made during
  admission, not an atomic transaction spanning a remote revocation service and
  SQLite.
- A matching retry after an allocation exists returns its current accepted
  status without re-verification, by the semantic-idempotency rule in
  `SPEC-flip-rpc`; it creates no new row and is outside the claim's creation
  quantifier.
- Admin remediation, restore, direct database modification, and in-crate
  `cfg(test)` contexts that construct `DaemonContext` with a test verification
  provider are not official public `RequestLiquidity` allocation creation and
  are outside the quantified writer domain.

## Weakest links

1. **L1 (`enum`/`code`)** — a new allocation writer or bypass is the main
   regression risk.
2. **L2 (`code`/`test`)** — gate order and all rejection exits need review when
   verification changes.
3. **L3 (`code`)** — the invite-preview/request identity join is runtime logic.
4. **L4 (`schema`/`code`)** — transaction atomicity and primary-key behavior.
5. **A1–A3 (`axiom`)** — cryptography, trust transports, and SQLite bottom out
   outside this record.
