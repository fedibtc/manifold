# Independent assessment — FMan dashboard audit

**Date:** 2026-08-08
**Subject:** `PRODUCTION-READINESS-AUDIT-FMAN-2026-08-08.md`
**Audit snapshot:** `0539876b` · **Assessment snapshot:** `e5923fd0` ·
**Re-checked at head `9b8e12b6`:** gate numbers and all cited code claims
unchanged; the two intervening commits are e2e/mock-control work (see the
isolation finding below). Where re-run numbers differ from the audit's, the
delta may reflect branch movement between snapshots, not audit error.
**Method:** every claim independently re-verified against the Rust and
TypeScript source; lint gates re-run (exact commands and outputs in the
assessment index appendix); git history and specs checked to separate live
requirements from stale documents.

## Verdict on the audit

**Agree: the FMan dashboard is not production-ready.** The audit's citations
check out. Its main weaknesses: it presents a spec-documented, tracked daemon
ask as a discovery; it misses two facts that change the shape of the fix
(the wizard's error-string trigger and the packaging depth); and it buries a
destructive-save defect inside a P2 grab-bag that deserves P1 on its own.

## Scorecard

| Audit finding | Audit | Verdict | My severity |
| --- | --- | --- | --- |
| Fresh-install browser onboarding unreachable | P1 | Confirmed — but a known, spec-tracked gap | P1, with corrections |
| Auth expiry hidden by cached data | P2 | Confirmed; non-401 failures collapse into `NetworkError` | P2 |
| Query failures rendered as valid states | P2 | 4/4 confirmed; `useOfferForm` is far worse than stated | Split: one P1, rest P2–P3 |
| Coverage not measured/enforced | P2 | Confirmed and understated — **zero frontend CI at all** | **P1** (shared, own once) |
| Browser-test isolation risk | P2 | Plausible per the audit's own run evidence; not re-reproduced | P2 |
| Shared form/stepper accessibility | P3 | Confirmed, one nuance | P3 |
| Styling/duplication/test structure | P3 | Mostly confirmed; boot.spec complaint refuted | P3 |

## Findings, re-verified

### P1 — Fresh onboarding unreachable: confirmed, but re-scope the fix

All four sub-claims hold: onboarding serves only the Unix socket
(`crates/fman/bin/src/main.rs:381-394`); HTTP admin binds only after the fleet
opens (`main.rs:495-549`) and is structurally incapable of pre-identity
operation (`admin_http::router` requires `Arc<Fleet>`); the wizard POSTs
`OnboardAsNew` to `/api/admin`; `packages/fleet-manager/entrypoint.sh` passes
no HTTP-admin flags. Over HTTP those onboarding verbs are always refused
(`admin.rs:338-340`).

Three corrections that matter for remediation:

1. **This is a tracked ask, not a new finding.** `MVP-SPEC.md:77-79,116-120`
   documents the exact gap, in the same words, under "Daemon asks". The audit
   should say so — it changes the conversation from "regression" to "known
   dependency not yet delivered".
2. **A bootstrap HTTP listener alone will not fix it.** The wizard triggers by
   string-matching `'has not been onboarded'` (`setupState.ts:8`) — an error
   only the socket path emits. The fix needs the listener **plus** the spec's
   asked-for read-only setup-state verb (or a stable error contract)
   (`MVP-SPEC.md:121-123`). The audit's remedial plan omits this and would
   fail acceptance as written.
3. **The packaging gap is wider than onboarding.** The entrypoint never
   enables HTTP admin in *any* state, so a packaged install has no dashboard
   even after onboarding. Both live paths that do work (dev stack `up.sh`,
   defe CI) onboard out-of-band over the socket first.

### P1 (upgraded) — Failed offer load enables a silent destructive save

The audit lists `useOfferForm.ts:51` as one bullet of "error is discarded"
(P2). The consequence is worse: `offer.isError` is never surfaced, so after a
failed load the form renders enabled, empty, and error-free — and
`OfferPage.tsx:26` documents that a **blank field stops selling seats**. One
click on Save converts a read failure into deleting the fleet's real price.
This is the highest-priority frontend fix in the FMan app.

### P2 — Auth expiry: confirmed

The `!data &&` gate is deliberate and documented in the hook
(`useBootStatus.ts:10-12`), and there is no global 401 handling anywhere —
`queryClient.ts` is a bare `new QueryClient()`, no `QueryCache` `onError`, no
interceptor. Fix at the API/query boundary.

One adjacent observation (not a defect today): the FMan client folds every
non-401 HTTP failure into `NetworkError` (`adminCall.ts:27-29`), which the
boot gate renders as daemon-unreachable. FMan's cookie-session auth appears to
use 401 exclusively, so this is currently harmless — but if the daemon ever
returns 403, it would surface as "daemon unreachable" rather than a
permission error. Worth one confirmation question to the Rust team (below);
per the FLIP spec's model, a 403 would deserve an access-denied state, not
re-authentication.

### P2 — Async states: confirmed with per-item severities

- `OverviewPage` — confirmed and worth spelling out: all inputs default to
  `[]`, so a **total API failure renders the green "Advertised and healthy"
  banner**; `useOverviewEarnings` computes `isLoading` and the page never
  reads it. P2.
- `SetupAuthorization` "Waiting" on loading/error — confirmed, but the query
  key is shared with the boot gate so a cached success usually exists. P3.
- `BackupPage` "—" for loading/failure — confirmed; low blast radius but a
  bad failure mode for a backup screen specifically. P3.

### P2 → P1 — No frontend verification runs in CI (shared finding)

The audit frames this as missing coverage thresholds. The measured reality:
`.github/workflows/selfci.yml` runs only `selfci check`, whose script defines
Rust jobs exclusively; `flake.nix` has no JS derivation. 143 unit-test files,
19 e2e specs, biome, the boundary linter, and a ready-made `test:e2e:ci`
script are all unwired. Wiring the existing scripts in is one cheap change and
should precede any coverage-threshold discussion. This is repo-level — it
should be owned once, not duplicated per product (see the index).

### P2 — Browser-test isolation: plausible, and likely already addressed

The 33/34 cross-case bleed and the stale port-8787 process match known
gotchas with this stack (the express mock server). Not independently
re-reproduced — and note that `9b8e12b6` ("drive e2e scenarios through the
in-browser mock control"), landed after both audit and assessment snapshots,
moves scenario control into the browser and away from the shared express
server, which plausibly removes both the cross-case bleed and the port
dependency. Re-run the full mocked suite at head before spending further
effort here; the remedial direction (isolated state, clear startup failure)
remains sound if it still reproduces.

### P3 — Accessibility: confirmed, with priorities

`TextInput`/`SelectField` lack `aria-describedby`/`aria-invalid`
(`data-invalid` is CSS-only) — a ~4-line mechanical fix each, since both
already compute `useId`. `FormField` is the one needing design: it exposes no
ids at all, so consumers *cannot* wire descriptions — schedule that first.
Stepper: no list semantics or `aria-current`, confirmed; narrow the color
claim — the current step also gets `font-bold`, the genuinely color-only
distinction is completed-vs-upcoming.

### P3 — Styling/test structure: mostly confirmed, one item dropped

Hard-coded values confirmed at all three cited sites. Setup-price duplicates
offer-price form machinery (~15 lines — offer extracted a hook, setup inlined
it). **Dropped: the boot.spec complaint.** Its loops are data-driven
assertions about a single page state; splitting them would re-boot and
re-authenticate per assertion and no repo convention forbids them.

## Revised FMan remediation order

1. Fix `useOfferForm` error handling (destructive-save path) — small and
   urgent.
2. Wire the existing frontend checks into CI (repo-level, coordinate via the
   index).
3. Resolve onboarding with the daemon team: pre-identity listener **plus** the
   setup-state verb, and expose HTTP admin in the packaged entrypoint.
4. Handle auth expiry (401) independently of cache at the query boundary.
5. Fix the overview green-on-failure derivation; then the P3 async states.
6. Accessibility: `TextInput`/`SelectField` mechanically, `FormField` as a
   scheduled interface change, stepper semantics.
7. Styling cleanups and the price-editor extraction.
8. Then measure coverage baselines and set thresholds.

## Additions to the Rust-team questions

The audit's question list is good. Add:

- Will the daemon provide a read-only setup-state verb (or a stable
  machine-readable error contract) so the UI stops string-matching
  `'has not been onboarded'`? (Without this, a bootstrap listener changes
  nothing for the wizard.)
- Should the packaged entrypoint enable `--admin-http-bind`/auth by default,
  and with what binding? (Today the packaged install has no dashboard at all.)
- Does the FMan admin API ever return 403? The client currently folds every
  non-401 failure into `NetworkError` (rendered as daemon-unreachable), so a
  permission denial would be mislabelled. If 403 exists, the UI needs an
  access-denied state — distinct from re-authentication.
