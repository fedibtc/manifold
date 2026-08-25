# Independent assessment of the 2026-08-08 production-readiness audits — index

**Date:** 2026-08-08
**Branch:** `feat/msw-mock-migration`
**Snapshots:** the audits were taken at `0539876b`; this assessment's deep
verification ran at `e5923fd0` (two commits later), and the lint gates and
cited claims were re-checked at the current head `9b8e12b6` — gate numbers
(biome 15 errors / 7 warnings / 8 infos, 1 boundary violation, 8 CSS-dupe
groups) and all cited code claims are unchanged between `e5923fd0` and
`9b8e12b6`. Numeric deltas between the audits and this assessment (for
example biome 9 → 15 errors) can reflect branch movement between snapshots
rather than audit error, and are labelled as such where they occur. Findings
that are snapshot-independent (for example the Requests deletion, which
predates both snapshots) are distinguished from drift explicitly.
**Subjects:** the split audit set —
`PRODUCTION-READINESS-AUDIT-2026-08-08.md` (index),
`PRODUCTION-READINESS-AUDIT-FMAN-2026-08-08.md`,
`PRODUCTION-READINESS-AUDIT-FLIP-2026-08-08.md`
**Method:** every finding independently re-verified against the Rust and
TypeScript source by five parallel code reviews; lint gates re-run; git
history and specs checked to separate live requirements from stale documents.
The ownership split did not change the audit content, so all verification
results carry over to the per-product documents.

## Per-product assessments

- **FMan:** [AUDIT-ASSESSMENT-FMAN-2026-08-08.md](./AUDIT-ASSESSMENT-FMAN-2026-08-08.md)
  — verdict agreed (not production-ready). Onboarding P1 confirmed but it is a
  spec-tracked daemon ask, and the audit's fix is incomplete (the wizard's
  error-string trigger needs a setup-state verb; the packaged entrypoint never
  exposes HTTP admin at all). One finding upgraded to P1: a failed offer load
  silently enables a destructive save that deletes the fleet's price.
- **FLIP:** [AUDIT-ASSESSMENT-FLIP-2026-08-08.md](./AUDIT-ASSESSMENT-FLIP-2026-08-08.md)
  — verdict agreed (not production-ready), **but on two P1s, not five**.
  Backup transport and timestamps are fully confirmed blockers. The Requests
  P1 is a stale spec (feature deliberately deleted in `e32a0a40`); the
  withdrawal and advertisement P1s deflate because the Rust daemon enforces
  the invariants the frontend omits. Missed by the audit: the OCI image
  definition in `flake.nix` overrides the daemon's safe loopback default with
  `0.0.0.0` (registry publication not verified).

## Shared findings — own these once, not per product

1. **No frontend verification runs in CI (upgrade to P1).** Both audits file
   this under "coverage is not measured" per product. The measured reality is
   repo-level: `selfci` runs Rust jobs only; 143 unit-test files, 19 e2e
   specs, biome, the boundary linter, and a ready-made `test:e2e:ci` script
   are entirely unwired. Duplicating the finding per product invites both
   teams to assume the other owns it. Wire the existing scripts in first —
   one change, before any coverage-threshold discussion (the audits' advice
   to measure a baseline before picking a percentage is right).
2. **The mock layer is the de-facto contract, and it is hand-authored.** The
   backup envelope, timestamp encodings, and `relay_cursors` all diverge from
   `admin.rs`, and nothing compares them — the mocks make broken code look
   correct in dev and e2e. Rust-produced contract fixtures (or generated
   types) fix the whole class for both products at once.
3. **The `docs/operator-dashboards/` tree is untracked — ignored by this
   workstation's global Git configuration, not by repository policy — and
   stale.** Requests, `relay_cursors`, and the polling module all changed
   backend-side without the specs following. Either the MVP docs join the
   versioned contract or they stop being audit inputs.
4. **Shared-UI accessibility** (assigned to FMan by the audit index — fine):
   confirmed; see the FMan assessment for priorities (`FormField` needs an
   interface change; the others are mechanical).

## Assessment of the audit set

- **Citation accuracy: high.** Nearly every file:line reference checks out;
  a few path errors only (`downloadTextFile` location, the republish and
  allocations hook paths).
- **Severity calibration: weak.** It never checked whether the Rust backend
  enforces the invariants the frontend omits, so three FLIP P1s inflate; and
  it filed the total absence of frontend CI under "coverage", deflating its
  most systemic finding.
- **Freshness: two different problems.** The biome delta (9 at `0539876b` →
  15 at `e5923fd0`) is branch movement between snapshots, not an audit
  miscount — it indicts the missing CI gate, not the auditor. The Requests
  finding is different: it was wrong at the audit's own snapshot, because the
  deleting refactor (`e32a0a40`) predates it — git history was not consulted.
- **Novelty: overstated in one place.** The FMan onboarding P1 restates a gap
  the MVP spec already documents with the same file references, without
  saying so.
- **The split itself is reasonable** — ownership boundaries match the code —
  with the one correction above: the CI/coverage finding is repo-scoped and
  should not be duplicated per product.

## Status of the audit documents

The three audit files now carry a note pointing here: the per-product
assessments supersede their severity rankings and remediation orders, so that
developers do not work from two contradictory lists. The audits remain
valuable as the source of the raw findings and the Rust-team question banks
(minus the corrections noted per product).

## Appendix — verification method and commands

The "five parallel reviews" were five independent read-only code
investigations, each assigned one cluster of audit claims (FMan onboarding;
FLIP backup + timestamps; FLIP requests/withdrawal/advertisement;
auth/polling/binding; coverage/e2e/a11y/styling). Each review read the cited
Rust and TypeScript sources, the relevant specs, and git history, and
returned per-claim verdicts with file:line evidence, which were then
cross-checked and synthesized here.

Commands actually executed (at `e5923fd0`, re-run with identical results at
`9b8e12b6`; from `operator-ui/` unless noted):

| Command | Output |
| --- | --- |
| `npx biome ci .` | `Found 15 errors. Found 7 warnings. Found 8 infos.` (7 `assist/source/organizeImports`, 7 formatter, 1 `lint/suspicious/noAssignInExpressions` at `scripts/check-css-dupes.mjs:71`) |
| `npm run lint:boundaries` | `1 problem (1 error)` — `apps/liquidity-provider/src/features/settings/hooks/use-provider-config-form/useProviderConfigForm.ts:4` "feature may not import feature" |
| `npm run lint:css-dupes` | `8 duplicate @apply group(s) found` (two 3× groups: `font-mono`, `text-error`; six 2× groups, one within `ReviewStep.module.css`) |
| `node -e 'console.log(Date.parse("1721476800"))'` | `NaN` (confirms numeric-seconds strings fail `Date.parse`) |
| `git log --oneline` / `git show` (repo root) | Confirmed `e32a0a40` (requests deletion), `8add6355` (relay-cursor deletion), `cb1e9db9` (last FMan `admin_http` change); no onboarding/admin work on this branch |
| `git check-ignore -v docs/operator-dashboards/...` (repo root) | Resolves to the workstation's global gitignore, not a repo `.gitignore` |

Unit-test, typecheck, and build results were not re-run; the audit's own
evidence tables record them as passing and nothing in the verification
contradicted that.
