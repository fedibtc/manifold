# Underwriting verdict — `operator-ui/apps/fleet-manager`

- Repo `/Users/kc/Projects/decentralized-federations`, branch `master`, HEAD `ae82da57`.
- Date: 2026-08-13.
- Scope: `apps/fleet-manager/src/`, excluding `src/mocks/` (development-only, kept out of
  production bundles by a build-time guard). Calls followed into `operator-ui/packages/*`,
  `@tanstack/query-core@5.101.2`, `crates/fman` and `crates/nostr-clients`.
- Crew: a scanner/checker pair, plus three blinded specialists (coroner, courier, ops-drill) who
  could not read the pair's record, each other's output, or the coordinator's reasoning. Prior
  audits of this app in `operator-ui/*.md` were placed off-limits to every role, so convergence
  below is genuine and not a shared prior.
- Raw records (transport, not deliverable; safe to discard): `../fleet-manager-*-a2.md`.

**Would I underwrite this module?** Not as written. The engineering quality inside each file is
high — the error taxonomy, the never-show-zero discipline for money, the boot gate and the
restore ladder are all better than typical. The failures are not local defects. Three of the
seven jobs this dashboard exists to do do not work against the daemon it ships inside, and in
every case the mock world answered where the daemon would not, so the test suite reports success.
The exclusion I would write on the policy is **reachability**: this module has never been proven
against a real daemon, and its green CI is not evidence that it has.

## Convictions, ranked by real cost

| # | Issue | Tier | Cost |
|---|-------|------|------|
| 1 | [The operator's only way to move money out fails against every real daemon](withdraw-calls-a-verb-the-daemon-deleted.md) — **fixed `5dbd2090` + W1.1b**, but see below | blinded convergence (4 roles) + checked + coordinator-verified | Money-out is dead; three CI gates were widened by name to hide it |
| 2 | [First-run setup cannot be reached in a browser](setup-surface-unreachable-over-http.md) | blinded convergence (4 roles) + checked + coordinator-verified | ~15 files of unreachable code; no host can be set up; nothing ever asks for a phrase backup |
| 3 | ["Earned, all time" stops counting after 20 remittances per seat](earnings-all-time-total-is-capped-at-20-per-seat.md) | blinded convergence (2 roles) + coordinator-verified | A money total that silently understates and has no visible rule |
| 4 | [A guardian that fails after starting keeps showing its last-known health](seat-health-polling-stops-when-a-seat-starts-running.md) | testimony (1 role), mechanism verified, cost corrected | A dead guardian reads "Healthy" on an unattended dashboard |
| 5 | [The Seats page claims an empty fleet when the read failed](seats-page-claims-an-empty-fleet-when-the-read-failed.md) | checked + convergence (2 roles) | A positive false claim about the operator's own fleet |
| 6 | [A blip empties the earnings dashboard for the whole outage](overview-blanks-for-the-whole-outage.md) | checked + convergence (3 roles) | Blank instead of stale, until the daemon returns |
| 7 | [The Authorization tab polls a relay every 3 s forever](authorization-watch-polls-a-relay-every-three-seconds-forever.md) | checked + convergence (4 roles) | ~1,200 relay REQs + 2,400 admin calls per visible hour, unbounded |
| 8 | [Seat detail stops polling permanently after one failure](seat-detail-stops-polling-permanently-after-one-failure.md) | checked + convergence (2 roles) | Permanent error screen with no way forward |
| 9 | [Rewording one daemon sentence would silently break setup](setup-gate-branches-on-daemon-prose.md) | checked | Latent behind #2; becomes live the day #2 is fixed |
| 10 | [Six screens invented six answers to "data present, refresh failed"](no-disposition-for-answered-then-failed.md) | checked (systemic) | Shared root of #5, #6, #8 — fixing them singly leaves a seventh site free to invent an answer |
| 11 | [The polling-policy module most pollers ignore](polling-policy-module-most-pollers-ignore.md) | checked (systemic) | Shared root of #7, #8; also the unowned `retry: 3` behind #6 |

Closure note on #1, in two parts. `5dbd2090` deleted the broken control and closed the three gates
— the `AdminRequest` union is now exactly the generated type, and **no test in `operator-ui/`
excludes an admin verb by string name**. W1.1b then built the replacement: a `/payouts` screen
driving all five money verbs, with the payout destination gating both sweeps, the guardian-fee
path shown as the two steps it actually is, and `awaiting_cycle_msat` stated on every collection.

**Money-out is reachable again, and it is still not proven.** Every test behind it is mocked — the
unit tests stub `adminCall`, and `e2e/fman/payouts.spec.ts` is mock-tier. The `@live` rung-M3 spec
that would assert the balance changed **at the daemon** is **still open**, blocked on the FMan live
e2e tier (W0.2), and was deliberately not faked. Read #1 as "the 422 is gone and the verbs the
daemon declares are now called", not as "the operator can move money" — the reachability exclusion
below still covers this path.

Admission note on #4: it did not pass the crew's own gate — one role, no checker, no convergence.
I admitted it because I verified the mechanism directly at
`seatStatus.ts:37-45` and both call sites, and corrected its cost claim downward in the issue
file. Read it as a verified mechanism with a disputed operating regime, not as a peer of #1–#3.

## Found after the verdict, during remediation

Not crew findings, and not ranked with them. Recorded here so a reader inherits
the whole picture rather than only what this run's roles were pointed at.

- [The mock could answer any shape, and did](mock-answers-are-not-typed-against-the-contract.md)
  — **fixed `eb0f4052`.** `Verb` was `(payload: unknown) => unknown` and every
  verb hand-cast its payload, so nothing held the mock to the daemon's shapes.
  One required money field, `lifetime_remitted_msat`, was missing from the
  `GuardianFees` answer with `pnpm typecheck` green. Surfaced by the W0.1b scan
  on 2026-08-13, inside the `src/mocks/` exclusion below — the crew named that
  the mock answered where the daemon would not, and did not ask what checked the
  mock.

## Coverage

**Examined hard, checked adversarially.** Every acquittal resting on a cost figure or a
query-core behavioural claim was re-derived from the installed `query-core` build, not from the
scanner's word: 13 attacked, **10 upheld, 3 weakened, 0 overturned**. The upheld set includes the
jitter-per-seed design, the module-level `Map`s not being a render side effect, the error-streak
counter, the 15 s admin timeout not stacking behind the 5 s cadence, `refetchOnMount: false`
being load-bearing for the gates, the fan-out slot hand-off, the statically-analysable mock
guard, and `gcTime: 0` on all three secret-bearing mutations. These are **checked** acquittals —
the strongest this run produced, and they cover the module's most-loaded mechanisms.

**Examined and found genuinely good.** The transport error taxonomy (`errors.ts`), the
never-show-zero policy for amounts (`format.ts`, `federationBalance.ts`,
`useOverviewEarnings.ts`), the boot gate's 401 promotion, `WalletPage`'s stale-with-banner
handling, and the restore partial-failure ladder. The last of these is well-argued code that
cannot currently run (#2).

**Weakest surviving acquittals** — where I would look first if something is wrong that we missed:

1. **`useAuthorizationWatch` sharing `ONBOARDING_KEY` with a different `queryFn`.** Upheld on
   cost only; its *written* defense was struck as false. *Falsifier:* observe any keyless
   imperative refetch (`refetchQueries`/`invalidateQueries` with no key) running while the
   Authorization page is mounted — that inherits whichever `queryFn` was written last, and the
   acquittal's cost bound does not survive it.
2. **`gateSurface` + two effects in production route guards.** Survives on bundle cost; its
   design defense was ruled inadmissible, since `MockPanelMount` sits inside both the router and
   the query provider and can derive the same four surfaces with no shared store. *Falsifier:* a
   third production consumer of `gateSurface` appearing.
3. **`requestLimit` waiters are never cancelled.** Defensible as written — nothing is dropped,
   arrival time is the only variable, the queue drains inside one 15 s window. *Falsifier:* any
   path that queues more work than one 15 s window drains, at which point unmounted screens'
   queued calls delay live ones.

**Not examined at all** — see Exclusions. The single largest gap in this run's own method:
the scanner judged decisions file by file and never asked, for any flow, whether the transport
can deliver the state that flow branches on. It caught the one instance where a test had been
amended (#1) and missed the larger one where no test was amended (#2) — the checker found that.
A reachability sweep is the cheapest addition to the next run.

## Exclusions

Flaw classes no fielded role examined. Residual uncertainty here is not covered by anything above.

- **Attacker-shaped reads.** Standing exclusion until an adversary role exists. Two independent
  systems have previously acquitted a known exploitable flaw because its conservation arithmetic
  read as safety. The auth-surface tickets in Leads below are ops-drill by-products, **not a
  security audit**, and must not be read as one.
- **Accessibility.** No role examined keyboard traps, focus order, screen-reader semantics or
  contrast. The recently landed update takeover is a full-screen dialog with, by its own PR
  notes, no tab trapping.
- **Design-system conformance.** No role checked the UI against the Fedi Design System or the MVP
  wireframes.
- **The e2e suite.** `operator-ui/e2e/fman` sits outside the module path and was untouched. Given
  #1 and #2, what the `@live` tier actually covers is an open question, not a settled one.
- **Sibling app and shared packages.** `apps/liquidity-provider` was not examined;
  `packages/common-ui`, `packages/mock-devtools` and `packages/types` were read only where this
  module calls them.
- **Daemon correctness.** `crates/fman` was consulted only for boundary facts (the verb list,
  startup ordering, the guardian-fee limit default, the error-kind axis). Nothing about daemon
  behaviour beyond those points is underwritten here.
- **Bundle size, render performance, and memory beyond the noted unevicted maps.**
- **Internationalisation, and multi-tab concurrency.**
- **`src/mocks/` and `mock-server/` as artefacts.** Judged only for what they cause the tests to
  prove, not for their own quality. A defect sat in this gap and was found later —
  [the mock could answer any shape](mock-answers-are-not-typed-against-the-contract.md).

## Leads

Unpromoted; carried so a re-run inherits them rather than starting cold. Not convictions.

**Auth surface (out of module — `crates/operator-ui-auth`, `crates/fman/core/src/admin.rs`;
found incidentally by the drill, never adversarially reviewed):**
- No rate limiting, throttling or logging on password attempts; cost per attempt is one
  constant-time compare, so the rate is bounded by the HTTP server, not by policy.
- The session cookie sets `http_only` and `same_site` but never `secure`, and the daemon accepts
  any bind address — so a non-loopback bind can carry password and session in cleartext.
- The cookie name is 4 random bytes redrawn per process, so each restart strands the previous
  cookie; no session expiry and no logout exist at all.
- In trusted-proxy mode a proxy 401 strands the operator on a sign-in form whose endpoint is not
  mounted, which answers 404 forever.

**Client, unpromoted:**
- ~~The withdrawal response's bearer ecash token renders with no copy control, unlike every other
  copyable identifier on the same screens, and is destroyed by leaving the screen.~~ Moot: the
  screen was deleted in `5dbd2090`. The replacement model has no bearer token to copy.
- The dashboard can only be served at an origin root — absolute `/api/*` paths, no router
  `basename`, no Vite `base` — and a path-prefixed deployment fails in a way that reads as a dead
  daemon.
- `share_matches_policy`, the field the spec designates for "this federation is cutting you out
  of the fee split", is fetched on every poll and rendered nowhere (courier).
- `unparsable_request` is a discriminant that cannot reach a browser (courier).
- `pollingIntervals.ts` keeps module-level `offsets`/`streaks` maps that are never evicted — a
  slow leak bounded by lifetime seat count, not material at target fleet sizes.
- `adminCall` never forwards react-query's `AbortSignal`, so cancellation is a no-op (folded into
  #11 but listed here as its own two-line fix).

**Cross-module, for the next global pass:**
- Check whether the sibling app's type mirror carries the same kind of by-name exception that hid
  #1, and whether the generator can fail closed on any hand-added variant.
- Both per-seat fan-outs point at the same missing daemon primitive
  (`operator-ui/docs/daemon-aggregate-verbs.md`); if FLIP has the same N+1, the fix is one
  aggregate-verb policy, not two client-side limiters.
- `describeActionError` exists in both apps with divergent taxonomies; `AdminApiError.reason` may
  now make them convergeable into one shared describer.
- `truncateMiddle`/`isTruncated` are called with hardcoded widths at four sites here; `common-ui`
  may want two named policies instead.
- ~~`crates/fman` has seven verbs no dashboard calls (`DecommissionSeat`, `CollectGuardianFees`,
  `SweepGuardianFees`, `SweepPaymentFees`, `PayoutDestination`, `SetPayoutDestination`,
  `ReenrollTelemetry`).~~ Down to two after W1.1b: the five money verbs now have callers.
  `DecommissionSeat` and `ReenrollTelemetry` remain uncalled. The drift question stands — a verb
  the daemon declares and no screen calls is still worth one pass.

## Re-run rules

A later change at an issue's cited sites reopens it. Ruled and accepted-residual issues are not
re-litigated without new evidence at those sites. The acquittals above go stale when their scope
changes — not when unrelated code moves.
