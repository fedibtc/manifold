# First-run setup — phrase, restore, authorization — cannot be reached in a browser

- **Status:** fixed — `c7e8b419` (`feat/fman-pre-onboarding-http`)
- **Tier:** blinded convergence (4 roles) + checked + coordinator-verified at the Rust source
- **Level:** code (client), with a named daemon dependency (pre-onboarding HTTP)
- **Found by:** checker (the scanner missed it), coroner, courier, ops-drill
- **Where:**
  - `operator-ui/apps/fleet-manager/src/app/components/setup-gate/SetupGate.tsx:22`
  - `operator-ui/apps/fleet-manager/src/features/setup/**` (wizard, doors, phrase, restore + 3
    result screens, authorization, price, `useSetupWizard`, `refreshIdentity`, `restoreViewState`,
    `setupState`) — ~15 production files
  - reachability decided by `crates/fman/bin/src/main.rs:487-502` and `:642-694`
  - `crates/fman/core/src/fleet.rs:421-424`, `crates/fman/core/src/onboarding.rs:124`

**What happens:** `SetupGate` opens the wizard only when a query fails with the daemon's
"has not been onboarded" refusal. A host with no identity serves that refusal on the **unix
admin socket only**: `main.rs:487-502` runs `Onboarding::run(&admin::socket_path(…))` and blocks
there. The HTTP listener is built later, at `main.rs:642-694`, from an already-open `Fleet` —
and `Fleet::open` refuses to produce one without an identity. The dashboard itself is served by
that same router. So a host with no identity has no HTTP listener and no dashboard; a host with
an identity never issues the refusal.

**The result:** `isSettingUp` can never become true in production. A fresh operator pointing a
browser at the daemon gets connection-refused, which the client classifies as
"daemon unreachable" — not a setup prompt. Roughly 15 production files, including three of the
module's most intricate decisions (the restore partial-failure ladder, `refreshIdentity`'s
direct fetch, the wizard latch), are maintained against a state no daemon can present. Their
entire test suite passes because `src/mocks/world/verbs.ts:312` serves the refusal the daemon
cannot — so the green suite is evidence about the mock, not the product. Downstream: nothing on
any reachable screen ever asks the operator to write down the recovery phrase the whole fleet
depends on.

**Failed defense:** None was offered — the scanner's inventory rated this flow the
best-reasoned code in the module without asking whether anything can run it. The available
defense, "pre-onboarding HTTP is planned"
(`operator-ui/docs/plans/2026-08-12-fman-dashboard-live-e2e-plan.md:226,492`), is the same
provenance argument rejected for the withdraw verb, and it is not a defense of the tree at HEAD.

**Fix direction:** Land pre-onboarding HTTP on the daemon before the client ships the surface
that depends on it. If the client must ship first, put the wizard behind an explicit, stated
"not reachable yet" marker rather than behind a condition that silently never fires — an
unreachable flow that looks exercised teaches every future reviewer the wrong thing. Whichever
lands first, the gate branches on a discriminant, not on prose (see
[`setup-gate-branches-on-daemon-prose`](setup-gate-branches-on-daemon-prose.md)).

**How it was fixed.** The packaging question this waited on was answered first: Umbrel has no
install-time user input, so the dashboard is the only place an operator can supply a phrase, and
that is also what Umbrel's own guidance recommends. StartOS could ask, but a phrase in a platform
config form is a mnemonic at rest in that store, so the dashboard is preferred there too. The ~15
production files stay and become reachable.

The daemon now binds the operator listener in the pre-identity phase, and hands it to the fleet
**in place** rather than binding a second one. `OperatorPhase`
(`crates/fman/core/src/admin.rs`) holds which dispatcher the listener answers from;
`OperatorPhase::open_fleet` switches it when `Fleet::open` returns. The embedded dashboard is
merged into that one router, so the wizard has something to load, and an operator who has just
answered the last question does not reload against a port that went away. Authentication moved
ahead of onboarding rather than changing: the password file is written by the deployment, so
nothing about it ever waited on a fleet. The unix socket is untouched and serves the same phase
concurrently, through the same `Onboarding`, so onboarding still happens once whichever transport
carries it.

One thing this exposed that the issue did not name: between the operator's answer and the fleet
opening, the phase must report neither the running answers nor `not_onboarded`. Reporting
`not_onboarded` there is false, and would send a browser that had just finished the wizard back to
its first screen.

**Observed red.** Deleting `OperatorPhase::onboarding` — the only way to build a listener without
a fleet, which is the fault exactly — stops `tests/admin.rs` compiling:
`no associated function or constant named 'onboarding' found for struct 'admin::OperatorPhase'`.
Making `open_fleet` a no-op instead leaves the listener answering, but the post-handover
`ShowPlans` fails with
`AdminError { kind: Other, message: "this Fleet Manager has been onboarded and is starting; its fleet is not open yet" }`
— the same address, no longer the fleet's. Both restored.
