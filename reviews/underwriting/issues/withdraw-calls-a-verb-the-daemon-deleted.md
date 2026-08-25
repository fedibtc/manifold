# The operator's only way to move money out fails against every real daemon

- **Status:** fixed — `5dbd2090` (W1.1a + W0.1) removed the broken control; W1.1b built the
  replacement on the daemon's own verbs. **The dashboard has a money-out surface again, and it is
  unproven against a real daemon** — see *Replacement (W1.1b)* below for exactly what is covered
  and what is not.
- **Tier:** blinded convergence (4 roles) + checked + coordinator-verified at the Rust source
- **Level:** code
- **Found by:** scanner, checker, coroner, courier, ops-drill
- **Where:**
  - `operator-ui/apps/fleet-manager/src/features/wallet/api/hooks/use-withdraw/useWithdraw.ts:20-23`
  - `operator-ui/apps/fleet-manager/src/features/wallet/components/federation-table/FederationTable.tsx:53`
  - `operator-ui/apps/fleet-manager/src/pages/wallet-withdraw/WalletWithdrawPage.tsx`
  - `operator-ui/packages/types/src/fleet.ts:227-236`
  - `operator-ui/packages/types/src/__tests__/contractFixtures.test.ts:257`
  - `operator-ui/apps/fleet-manager/src/mocks/__tests__/verb-catalogue.test.ts:15`
  - daemon side: `crates/fman/core/src/admin.rs` (no `Withdraw` variant), `crates/fman/core/src/admin.rs:128-132`

**What happens:** The withdraw screen POSTs `{ Withdraw: { federation_id, amount_msat } }` to
`/api/admin`. The daemon's `AdminRequest` enum has no `Withdraw` variant — it was deleted in
favour of `PayoutDestination` / `SetPayoutDestination` / `SweepPaymentFees`. The HTTP adapter
extracts with `Json(request): Json<AdminRequest>`, so an unknown externally-tagged variant is
rejected by the extractor with HTTP 422 before dispatch ever runs. The operator sees "The fleet
manager refused the request (HTTP 422)."

**The result:** There is no working way to withdraw funds from a payment federation, and there
has not been since the verb was deleted. The five verbs that actually move money
(`PayoutDestination`, `SetPayoutDestination`, `SweepPaymentFees`, `CollectGuardianFees`,
`SweepGuardianFees`) have no UI at all. No CI tier can see the hole, because all three gates
that would have caught it were widened for this exact variant: `packages/types/src/fleet.ts`
hand-adds the verb back onto the union generated from Rust, `contractFixtures.test.ts:257`
excludes it by name, and `src/mocks/world/verbs.ts` implements it — so the mock answers where
the daemon 422s, and the whole test suite is green.

**Failed defense:** "It is interim, it is documented, and the payout-destination wave removes
the screen and the exclusions together" — the comment at `fleet.ts:227-233` says exactly this.
That is provenance, not merit. It is also self-defeating on effort: deleting the route, the row
link and the hook is *less* work than hand-patching a generated union and maintaining two
exclusion lists. A defense that explains why the broken thing still exists is not a defense of
shipping it.

**Fix direction:** While the daemon has no withdrawal verb, the dashboard has no withdrawal
action — remove the screen, the row link and the hook. Restore money-out on top of
`SetPayoutDestination` / `SweepPaymentFees` when that wave lands. The contract-mirror test keeps
no named exceptions: an exception by name is how a deleted verb survived three gates.

## Fix (`5dbd2090`)

The deletion (W1.1a): the `wallet/:federationId/withdraw` route, the `Withdraw` row link and its
`.rowAction` style, `useWithdraw`, `useWithdrawForm`, `WalletWithdrawPage`, the mock's `Withdraw`
verb and its entry in `MUTATING_VERBS`, and the `Withdraw` / `WithdrawResponse` types. The Wallet
intro no longer says "Withdrawals are per federation", because there are none.

The gate (W0.1): `AdminRequest` in `packages/types/src/fleet.ts` is now exactly
`GeneratedAdminRequest` — no hand-written member, and no `PendingAdminRequest` escape hatch, which
would be empty today and an invitation tomorrow. Both by-name exclusions are gone:
`contractFixtures.test.ts` keys its mirror on `AdminRequestName` itself, and
`verb-catalogue.test.ts` asserts the mock answers *nothing* beyond the daemon inventory. No test
in `operator-ui/` now excludes an admin verb by string name.

At that point money-out had no dashboard surface at all: the five verbs that move money had zero
callers under `operator-ui/apps/`, and the operator moved funds with the CLI. `5dbd2090` closed
*the 422*, not the gap.

### Observed red

The gate was falsified from every direction a fake verb could enter, then reverted:

1. `| { Bogus: Record<string, never> }` added to `AdminRequest` in `fleet.ts` →
   `pnpm --filter @operator-ui/types typecheck` red with
   `error TS1360: ... Property 'Bogus' is missing in type '{ ShowPlans: ... }' but required in
   type 'Record<AdminRequestName, AdminRequest>'`.
2. `Bogus: { Bogus: {} }` then added to the mirror to satisfy `tsc` → the runtime mirror test red,
   two failures: `expected { Object (CollectGuardianFees, DecommissionSeat, ...) } to deeply equal
   { Bogus: { Bogus: {} }, …(18) }` and `expected [ 'CollectGuardianFees', …(17) ] to deeply equal
   [ 'Bogus', …(18) ]`.
3. `Bogus` added to the mock verb map → `verb-catalogue.test.ts` red with
   `expected [ 'Bogus' ] to deeply equal []`.
4. `Bogus` written into the generated union instead → `cargo test -p fman-core --test
   admin_request_ts` red with `the committed TypeScript AdminRequest union is stale — run
   `just gen-contract-fixtures``.

All four are green on the committed tree. Before this change, step 1 could not fail: the union
carried the hand-written member and both tests named it in an exclusion.

The mock-tier `e2e/fman/wallet.spec.ts` now asserts the federation row offers no add, remove or
money-out control, so the dead link cannot come back unnoticed.

## Replacement (W1.1b)

A `/payouts` screen now drives all five money verbs, in the order the daemon enforces:
`operator-ui/apps/fleet-manager/src/pages/payouts/PayoutsPage.tsx` over
`src/features/payouts/`. The two revenue sources are deliberately not one list, because they are
not one shape.

**What it covers.**

- **The destination first.** `PayoutDestination` / `SetPayoutDestination` sit above both revenue
  sections. With none stored the card says so and every sweep control is disabled with the reason
  on screen, so the ordering `fleet.rs:1130` enforces is read rather than discovered through a
  refusal. Collecting is *not* gated on it — a collection moves money inside the fleet, and the
  daemon asks for no destination.
- **Setup-payment revenue**, per federation, one step: `SweepPaymentFees` with the federation id
  and nothing else. No amount field and no gateway picker, because the admin API exposes neither
  (`admin.rs:63`); the sweep takes the largest economically fundable amount.
- **Guardian-fee revenue**, per seat, two steps, presented as two numbered buttons:
  `CollectGuardianFees` then `SweepGuardianFees`. A collection's confirmation always names
  `awaiting_cycle_msat` beside `claimed_msat`, including when it is zero, so it can never read as
  "the account was emptied".
- Amounts follow the standing rule: an unread balance or unread fee account renders `—`, never a
  zero, and an unread figure never disables a control — the daemon stays the authority on whether
  there is anything to move.
- Reads go through `useQueryDisposition` / `QuerySurface`, so an outage marks the screen stale
  rather than blanking it. Per-seat fee reads sit outside that disposition: one seat's fee account
  failing is a fact about that row.

**What it does not cover — the exclusion still stands.**

- **Nothing here has run against a real daemon.** The unit tests mock `adminCall`; the e2e spec
  (`operator-ui/e2e/fman/payouts.spec.ts`) is mock-tier, driving MSW. A mock cannot falsify a
  request shape the daemon would reject, which is the exact failure mode this issue is about. The
  request shapes were read off `crates/fman/core/src/admin.rs` by hand and are covered by the
  generated contract mirror, which is a weaker guarantee than a round trip.
- **The `@live` rung-M3 spec this issue asks for is still open**, and is blocked on W0.2: the FMan
  live e2e tier does not exist yet. It must set a destination, sweep, and assert the balance
  changed **at the daemon** — not in the UI. Until it runs, treat the screen as "calls verbs the
  daemon declares", not as "moves money".
- No `@live` spec was faked to close this. The mock-tier spec is named as mock-tier in its own
  header comment.
- The hostile review commit `197af2cd` asked for, on the new Lightning and state-machine edges
  this screen drives, is still outstanding (Wave 5).

### Observed red (W1.1b)

Each assertion was falsified by breaking the code under it, then reverted:

1. `describeCollection` reduced to `Claimed ${sats}.` — dropping `awaiting_cycle_msat`, the figure
   most likely to be silently omitted. Four failures across three tiers:
   `expected 'Claimed 13,000 sats.' to be 'Claimed 13,000 sats. 3,000 sats stay …'`,
   `expected 'Claimed 13,000 sats.' to be 'Claimed 13,000 sats. 0 sats are waiti…'`,
   `Unable to find an element with the text: /3,000 sats stay locked until the next cycle
   turnover/`, `Unable to find an element with the text: /0 sats are waiting for the next cycle
   turnover/`; and the e2e red with
   `Error: expect(locator).toBeVisible() failed / Locator: getByText(/3,000 sats stay locked until
   the next cycle turnover/) / element(s) not found`.
2. The `!hasDestination` branch removed from `PaymentSweepAction` — three unit failures
   (`expect(element).toBeDisabled()` at `PaymentSweepAction`, `PaymentSweepTable`, `PayoutsPage`)
   and the e2e red with `Expected: disabled / Received: enabled`.
3. `describePayout` reduced to a constant — `expected 'Sweep sent.' to be 'Sent 250,000 sats.'`
   plus both settled-amount assertions red.
4. `onClear` sending `''` instead of `null` — `- "destination": null, + "destination": ""`.
5. `useSweepGuardianFees` calling `CollectGuardianFees` — `- "SweepGuardianFees": Object {
   + "CollectGuardianFees": Object {` in both the hook test and the component test.

All green on the committed tree: 561 unit tests and 48 mock-tier e2e specs.
