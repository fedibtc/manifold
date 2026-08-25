# The mock could answer any shape, and did — one required money field was missing

- **Status:** fixed `eb0f4052`
- **Tier:** none. Found during remediation of this verdict, not by the crew — see
  Provenance below. No blinded role examined it and no checker attacked it.
- **Level:** code (test infrastructure, but it decides what every other test proves)
- **Found by:** the W0.1b remediation scan, 2026-08-13, after the verdict was
  written
- **Where:**
  - `operator-ui/apps/fleet-manager/src/mocks/world/verbs.ts:4` (was
    `export type Verb = (payload: unknown) => unknown`)
  - the same file at `:42,48,66,79,99,113,128,141,153,174,187` (eleven
    hand-cast payloads)
  - `operator-ui/packages/types/src/fleet.ts` (`GuardianFeesResponse` declares
    `lifetime_remitted_msat` required)
  - `operator-ui/packages/types/fixtures/fman_guardian_fees.json` (the generated
    fixture carries it)

**What happens:** The contract has two edges and they were guarded very
differently. Daemon to types is strong: `crates/fman/core` generates 27 response
fixtures, `contractFixtures.test.ts` mirrors each as a typed literal with
`satisfies`, and a runtime assertion stops the mirror drifting from the
generator. Types to mock was guarded by nothing, in both directions. `Verb` was
`(payload: unknown) => unknown`, so a mock answer of any shape compiled. Every
verb hand-cast its own payload — `payload as { seat_id: string }` and ten more —
so a field renamed in Rust left the mock destructuring the old name, green.

**The result:** the mock's `GuardianFees` answer omitted
`lifetime_remitted_msat`, a required field on `GuardianFeesResponse` that the
generated fixture carries. `pnpm typecheck` passed. Every unit and mock-tier e2e
run in this app reads that answer, so the module's whole suite was proving a
shape the daemon does not send. Whoever wired the client half of
[the lifetime-earnings work](earnings-all-time-total-is-capped-at-20-per-seat.md)
would have read `undefined` off the mock, seen a screen that looked right, and
shipped it — the exact failure mode this review exists to describe, one layer
below where the review looked for it.

**Failed defense:** "the mock is dev-only, so its types do not matter." The
opposite holds. The mock is not shipped, but it is what the tests believe the
daemon is, so it is the one artefact whose fidelity every other piece of
evidence in this module rests on. A mock free to answer any shape converts a
green suite into a statement about itself.

**Fix direction (done):** join the two halves that were already declared but
never connected. `AdminRequestPayload<N>` reads the payload off the generated
`AdminRequest` union; `AdminResponseByName` names each verb's declared response,
kept exhaustive against `AdminRequestName` by a type-level assertion. `Verb<N>`
is the pair, the verb table is keyed by `AdminRequestName` so a verb added in
Rust fails to compile until it is answered, and the eleven hand-casts are gone.
One narrowing remains, at the single point where wire JSON meets the table.

Watched fail before it was trusted: deleting `lifetime_remitted_msat` from the
mock answer gives `TS2322 ... Property 'lifetime_remitted_msat' is missing ...
but required in type 'GuardianFeesResponse'`, and renaming one destructured
request field gives `TS2339 Property 'seatId' does not exist on type
'{ seat_id: string; }'`. Both reverted; `pnpm typecheck` green after.

## Provenance

This is not a crew finding, and it should not be counted as one. The verdict's
scope line excludes `src/mocks/`, and its exclusion list says the mocks were
"judged only for what they cause the tests to prove, not for their own quality".
That exclusion was reasonable and it was also where the defect sat: the crew
correctly identified that the mock answered where the daemon would not, then did
not ask what held the mock to the daemon's shapes. Nothing did.

Recorded here because a reader of the verdict should be able to see the whole
class, and because the same question — what checks the stand-in? — is unanswered
for `apps/liquidity-provider`, which this run did not examine either.
