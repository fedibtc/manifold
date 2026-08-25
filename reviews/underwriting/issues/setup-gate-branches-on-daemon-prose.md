# Rewording one daemon sentence would silently break first-run setup, with every test green

- **Status:** fixed — `c7e8b419` + `c3d9a6e3` (`feat/fman-pre-onboarding-http`)
- **Tier:** checked
- **Level:** code (client) + a one-variant daemon contract change
- **Found by:** checker (regenerated inventory; the scanner never listed the file), courier
- **Where:**
  - `operator-ui/apps/fleet-manager/src/features/setup/utils/setupState.ts:7-10`
  - consumed by `.../app/components/setup-gate/SetupGate.tsx:22` and
    `.../features/setup/components/setup-restore/SetupRestore.tsx:125`
  - producer: `crates/fman/core/src/onboarding.rs:124`
  - the axis it should use: `crates/fman/core/src/admin.rs:209-248`, mirrored at
    `operator-ui/packages/types/src/fleet.ts:335-343`

**What happens:** The single branch deciding whether an operator ever sees the setup wizard is a
substring match on daemon prose — `'has not been onboarded'` — against an error whose structured
`kind` is `other`. The daemon carries a discriminant axis, `AdminErrorKind`, which exists (in its
own doc-comment) precisely so the browser wizard need not match prose. It has no `not_onboarded`
variant, so the refusal arrives undifferentiated and the client sniffs the sentence.

**The result:** Today, nothing — the branch is unreachable
([`setup-surface-unreachable-over-http`](setup-surface-unreachable-over-http.md)). The day that
is fixed, this becomes a live trap: any reword of the refusal sends a fresh host into a dashboard
where every panel fails and no wizard opens. Nothing fails first. The mock reproduces the
sentence verbatim, so every client test passes on a hand-copied string, and the one Rust test
that pins the substring exists for the CLI and records no browser dependency — a reword updates
both together and CI stays green.

**Failed defense:** The file's own comment concedes the shape, calling it "the inference the
daemon ask exists to remove". A tracked ask is not a defense of the code that waits on it.

**Fix direction:** Add `not_onboarded` to `AdminErrorKind` — the enum's doc-comment describes
exactly this procedure, and the generated fixture mirror makes the client half automatic — then
branch on the discriminant instead of the sentence.

**How it was fixed.** Shipped with the daemon change that makes this branch reachable
([`setup-surface-unreachable-over-http`](setup-surface-unreachable-over-http.md)), because it goes
live the day that lands. `AdminErrorKind::NotOnboarded` was added and the onboarding refusal became
a type (`crates/fman/core/src/onboarding.rs::NotOnboarded`) so the discriminant survives the `?`
that boxes it into `anyhow`. The generated fixture and the TypeScript union followed;
`setupState.ts` reads `error.reason === 'not_onboarded'` and nothing in the client matches daemon
prose. The mock answers with the kind, and the tests that stand in for the daemon deliberately
carry a *different* sentence from the one it ships — a test that copies the prose would rebuild the
coupling being removed.

**Observed red, both sides of the fix.** The reword is the whole argument, so it was run twice with
the same sentence — "nobody has set this Fleet Manager up; choose a new fleet or recover an old
one" — applied to the daemon and the mock together, as a reword commit would.

*Before the fix,* on `819d149c` with that reword and the two by-name assertions updated alongside
it (`tests/onboarding.rs`, `mocks/__tests__/handlers.test.ts`), the suite is **entirely green**: 6
Rust onboarding tests, 561 client tests, 99 files. Nothing fails. But the wizard no longer opens:
teaching `SetupGate`'s stub to say what the daemon now actually says turns 3 of its tests red with
`TestingLibraryElementError: Unable to find role="heading" and name "Set up your fleet manager"` —
a fresh host lands in a dashboard where every panel fails and no wizard appears.

*After the fix,* the identical reword changes nothing: 9 Rust tests and 148 client tests across
setup, the gate, boot and the mocks all pass untouched.

Independently, reverting `AdminError::from_error`'s classification so the refusal reports `other`
turns 4 Rust tests red (`left: Other, right: NotOnboarded`), and restoring the old substring match
in `setupState.ts` turns the gate's own tests red on both the reworded refusal and the impostor
that reads like one. All restored.
