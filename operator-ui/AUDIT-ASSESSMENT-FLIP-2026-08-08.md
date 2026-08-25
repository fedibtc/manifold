# Independent assessment — FLIP dashboard audit

**Date:** 2026-08-08
**Subject:** `PRODUCTION-READINESS-AUDIT-FLIP-2026-08-08.md`
**Audit snapshot:** `0539876b` · **Assessment snapshot:** `e5923fd0` ·
**Re-checked at head `9b8e12b6`:** gate numbers and all cited code claims
unchanged (the two intervening commits are FMan e2e/mock work and touch no
FLIP code). Where re-run numbers differ from the audit's, the delta may
reflect branch movement between snapshots, not audit error; such cases are
called out explicitly.
**Method:** every claim independently re-verified against the Rust and
TypeScript source; lint gates re-run (exact commands and outputs in the
assessment index appendix); git history and specs checked to separate live
requirements from stale documents.

## Verdict on the audit

**Agree: the FLIP dashboard is not production-ready — but for two of the five
claimed P1s, not five.** The backup-transport and timestamp findings are fully
confirmed and are the real blockers. The other three P1s do not survive
contact with the Rust backend or with git history: the audit reviewed the
React layer in isolation and read every missing client-side guard as a missing
guard, period, and it treated a deleted feature's stale user story as a live
requirement. It also missed the most serious deployment fact in its own
network-locality finding.

## Scorecard

| Audit finding | Audit | Verdict | My severity |
| --- | --- | --- | --- |
| Backup/restore transport mismatch | P1 | Confirmed in full, and worse than stated | **P1 — worst finding** |
| Timestamp contract disagreement | P1 | Confirmed (crash + silent "—"); one overstatement | P1 |
| Requests workflow absent | P1 | **Stale spec — feature deliberately deleted** | Not a defect; doc fix |
| Withdrawal accepts invalid input | P1 | Frontend confirmed; **backend fully validates** | P2 + fix the test |
| Advertisement bypasses safeguards | P1 | Mostly refuted; only the confirmation-copy claim holds | P2 (copy), rest P3 |
| Auth expiry hidden by cache | P2 | Confirmed; also: 403 has no access-denied state | P2 |
| Polling/stale-data contract | P2 | Confirmed; drift is *broader* than stated | P2 |
| Admin network locality | P2 | Partially refuted — default **is** loopback; missed the OCI image-definition override | P2 (image: P1-adjacent) |
| Coverage not measured/enforced | P2 | Confirmed and understated — **zero frontend CI at all** | **P1** (shared, own once) |
| Quality gates not clean | P3 | Confirmed; biome at 15 errors on the later snapshot (see finding) | P3, gate-wiring P2 |

## Findings, re-verified

### P1 — Backup download is a path, not a backup (confirmed, strengthened)

Rust returns a daemon-local path as a transparent JSON string
(`backup.rs:50-55,271-298`; `service-liquidity-manager/src/types.rs:149-152`)
and **no byte-transport endpoint exists** — `admin_http.rs:81-83` is the whole
backup surface. The UI downloads that path string as `flip-backup-….txt`
(`BackupCard.tsx:15`) under copy falsely telling the operator it is needed for
recovery. Normal-mode restore is refused (`backup.rs:64-68,257-259`), so
`RestoreConsolePage` — the only real restore surface — asks for an archive it
can never receive. Three facts the audit missed that sharpen it:

- **The project's own spec flagged this exact question as a blocker before
  the screens were built** (`user-stories/12-backup-restore.md:237`: "Archive
  transport mechanics (blocker for implementation)… Must be resolved with the
  daemon team before CreateBackupCard/RestoreWizard are built"). It shipped
  anyway.
- **The mock server invents a self-describing JSON envelope**
  (`mock-server/src/routes/admin/backup.ts:46-50`) that round-trips
  download→paste→restore perfectly — the sole reason the flow looks green in
  dev and e2e. The unit test bakes the wrong model in
  (`'opaque-archive-contents'`).
- Path correction: the downloader is
  `features/settings/utils/downloadTextFile.ts`, not `shared/utils/`. And the
  TS *type* is actually honest (`admin.ts:289` says "opaque handle, serde
  transparent") — the defect is the UI's semantics, not a type mismatch.

The audit's remedial plan (binary download, streaming upload, cross-host live
test) is right.

### P1 — Timestamps: confirmed, with one overstatement

`Timestamp` is a transparent `u64` of Unix **seconds**
(`crates/domain/src/lib.rs:64-67`; `now_timestamp()` uses `as_secs()`). TS
declares `Timestamp = string` and `adminCall` bare-casts `response.json()` —
no normalization layer. Verified consequences:

- Attestations panel **crashes in render**: `formatDate` calls `.slice(0,10)`
  on what the daemon sends as a number (`format.ts:14` ←
  `admin.rs:598-599` `ingested_at`).
- Advertisement dates silently render "—": `Date.parse` of numeric seconds is
  `NaN` (`advertisement/services/format.ts:13-17`), for both the raw number
  and a numeric string.
- **Overstatement:** not all formatting is broken —
  `overview/utils/time.ts:7-12` defensively accepts both forms, so Overview
  survives. The codebase has three timestamp-parsing strategies; that
  inconsistency is itself part of the problem.
- Drift confirmed: TS omits `background_workers` (compile-time only today —
  rendering goes through `humanizeToken`, so no runtime break yet) and still
  carries `relay_cursors`, which was deleted backend-side in `8add6355`; the
  mock still validates the stale 8-group shape.
- Mock fixtures are inconsistent *with each other*: health uses numeric-string
  seconds, advertisement/attestations use ISO-8601. Three encodings across
  backend, mocks, and fixtures.

### Dropped — "Required Requests workflow is absent" (audit P1)

The requests domain was **deliberately deleted** in `e32a0a40`
("refactor(flip)!: delete request_id; the federation is the allocation
identity") — request store, `list/get_liquidity_request`,
`AdminRequestStatus`, all removed by design. The cited user story
(`06-requests.md`, mtime Jul 14) predates that refactor (~Jul 22) and was
never revised; the current spec of record
(`crates/liquidity-manager-daemon/specs/SPEC-flip-admin-api.md`) has no
request domain. The whole `docs/operator-dashboards/` tree is untracked and
globally gitignored, so it carries no authority over the shipped API. Acting
on this P1 would rebuild an intentionally removed concept. Correct action:
mark `06-requests.md` and the MVP flow-3 row superseded, and delete the
audit's "Requests API" Rust-question block.

### Downgraded — Withdrawal input (audit P1 → P2)

Frontend claim confirmed: `Number(value) || 0`, no guard, and the unit test
certifies blank submission as success (`FundsActions.test.tsx:76-88`). But the
daemon independently rejects empty intents, zero amounts, and bad addresses
before any irreversible step (`funds_admin.rs:252-288`), catches insufficient
balance inside the insert transaction, and the intent-id binding makes repeat
submits idempotent. The UI surfaces the rejection banner. No fund-loss path
exists — the worst outcome is a wasted round trip and a raw error string. Fix
the UX, and above all fix the test: it encodes a nonsense interaction as
correct, so a future validation fix will read as a regression.

### Downgraded — Advertisement actions (audit P1 → one P2 + P3s)

- **`force: true` is not a bypassed safeguard.** Its entire backend effect is
  `clear_relay_states` (`advertisement.rs:156-158`) — republish to every
  relay instead of skipping fresh ones. That is the correct flag for a button
  labeled "Republish now". Drop the "reserve force for recovery" remediation.
- **Readiness is enforced server-side.** A not-ready publish persists the
  not-ready state and returns without publishing (`advertisement.rs:65-70`).
  Enabling the button regardless is a missing affordance (P3), not a bypass.
- **The confirmed defect:** `AdvertisementPage.tsx:120` promises "You'll be
  asked to confirm" and `handleWithdraw` mutates directly on click — no
  dialog component exists anywhere in the app. Copy promising an absent guard
  invites confident clicking; withdrawal hides the provider from every app's
  discovery until manual republish. P2. (Also: `withdraw.mutate(null)`
  discards the `reason` field the wire type supports.)
- Path correction: the republish hook is under
  `features/advertisement/hooks/use-republish-advertisement/`, not
  `.../api/hooks/`.

### P2 — Auth expiry: confirmed, plus an adjacent gap

No global 401 handling exists (`queryClient.ts` is a bare
`new QueryClient()`), and the `!data &&` gate is deliberate. The documented
acceptance criterion (`07-allocations.md:92`: poll 401 → re-auth gate) is
unmet.

Adjacent gap the audit didn't cover: the spec deliberately distinguishes 401
(missing/invalid bearer → re-authenticate) from 403 (`permission_denied` on
an authenticated request) — `SPEC-flip-admin-api.md:31-33`. The client
correctly does **not** treat 403 as a re-auth trigger (`adminCall.ts` maps it
to `AdminApiError` via the `ServiceError` body), but no boot gate or shared
state recognizes it either, so a first-load 403 falls through and renders the
dashboard. The right fix is an explicit access-denied state for 403 — not
mapping it to `AuthError`, which would wrongly bounce a permission-denied
operator to re-authentication.

### P2 — Polling: confirmed, and broader than stated

The contract is real — `02-requirements-baseline.md:108-116` (allocations 5s
while non-terminal; health/funds/advertisement 30s; setup 60s), reinforced in
three user stories and a wireframe. Actual state: allocations **list and
detail** never poll; funds, advertisement, **and health** poll at 60s (2× too
slow — the audit named only funds/advertisement); wallet-ops and setup are
correct. The mandated central polling module (NFR-03) does not exist.
`FundsPage.tsx:34` checks `isError` before `data`, so with `retry: false` one
failed poll tick blanks the page — violating three written "never blank"
rules (`MVP-SPEC.md:32`, `04-funds.md:22,45`). Correct predicate:
`!funds.data && funds.isError`. Path correction: the hook is
`features/allocations/api/hooks/use-allocations/useAllocations.ts`.

### P2 — Network locality: the audit got the code right and the risk wrong

The default bind **is** loopback (`config.rs:26-27`:
`127.0.0.1:8173`/`8174`) — the audit's "accepts an arbitrary SocketAddr"
framing omits that the out-of-the-box posture is spec-compliant. Validation is
genuinely absent (`0.0.0.0` binds silently, no warning, no TLS in the crate).
The finding the audit missed is the serious one: **the OCI image definition
sets `FLIP_ADMIN_BIND_ADDRESS=0.0.0.0:8173` and exposes both ports**
(`flake.nix:545-551`, asserted as intentional at `:612`; whether the image is
published to a registry was not verified) — a container built from it is in
exactly the state the spec forbids
(`SPEC-flip-admin-api.md:28-29`), guarded only by the operator remembering
`-p 127.0.0.1:…` (the docs do show it; a default `-p` or a Kubernetes Service
publishes the admin API). SECURITY.md is silent on FLIP network posture.
Remediation: fix the image default first, then add bind validation with an
explicit override, then document the boundary in SECURITY.md.

### P2 → P1 — No frontend verification runs in CI (shared finding)

Same as the FMan assessment: no frontend check of any kind runs in CI — not
coverage, *anything*. See the index; this should be owned once at repo level,
and it is the root cause of the mock/backend divergences above (nothing
compares the hand-authored mocks against `admin.rs`).

### P3 — Quality gates: confirmed, counts stale

Boundary violation confirmed exactly (`useProviderConfigForm.ts:4`, imports
`ADVERTISEMENT_KEY` across features); 8 CSS-dupe groups confirmed exactly
(one is a within-file dupe in `ReviewStep.module.css` — cheapest fix). Biome
was at **15 errors** on the assessment snapshot `e5923fd0` and identical at
head `9b8e12b6` (7 organizeImports, 7 formatter, 1 `noAssignInExpressions`);
14 clear with one `biome check --write`. The audit's count of 9 was taken at
`0539876b` — the delta most likely reflects the two MSW commits that landed
between the snapshots, not an audit miscount. Either way it demonstrates the
same thing: with no CI gate, the number moves commit to commit.

## Revised FLIP remediation order

1. Wire the existing frontend checks into CI (repo-level, coordinate via the
   index) and auto-fix the biome debt.
2. Fix the timestamp contract — numeric seconds is what Rust ships; add a
   normalization/codec layer at `adminCall`, collapse the three parsing
   strategies into one, and add Rust-produced contract fixtures that mocks
   and tests must consume (also fixes `relay_cursors`/`background_workers`).
3. Implement real backup byte transport per the already-written blocker note
   in `12-backup-restore.md:237`; until then, disable or clearly re-label the
   misleading download/paste UI.
4. Change the OCI image definition to a loopback default; add bind validation
   with an explicit override; document the boundary in SECURITY.md.
5. Add the withdrawal confirmation dialog (or delete the promise from the
   copy); fix `FundsPage`'s error-over-data predicate; align polling with the
   documented contract via the mandated shared module.
6. Handle auth expiry (401) independently of cache; add an explicit
   access-denied state for 403 (per the spec, 403 is a permission error on an
   authenticated request — it must not trigger re-authentication).
7. Add client-side withdrawal validation and replace the blank-submit test.
8. Retire `06-requests.md` and the other superseded docs.
9. Then measure coverage baselines, set thresholds, and deepen the live tier
   with write-path smokes.

## Changes to the Rust-team questions

- **Delete the "Requests API" block** — answered by `e32a0a40`; asking it
  re-opens a closed decision.
- Backup/restore, timestamp, and exposure questions are good; add: "Should
  the OCI image definition default to loopback, with port publication as the
  opt-in?"
- The withdrawal-invariant questions are already answered in
  `funds_admin.rs:252-352` (zero/empty rejection, balance check in the insert
  transaction, intent-id idempotency) — replace them with "confirm these are
  the committed invariants and add contract tests", which is cheaper than
  re-deriving them.
- Under `force: true`: the answer is in `advertisement.rs:156-158` (relay
  cache clear). The remaining real question is only whether it should ever be
  restricted.
