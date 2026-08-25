# An unauthorized Authorization tab drives 1,200 relay reads an hour, indefinitely

- **Status:** fixed in `7e0318f3` ("fix(fman-ui): decay the authorization watch
  and let the operator force it")
- **Tier:** checked + blinded convergence (4 roles)
- **Level:** code
- **Found by:** scanner, checker, coroner, courier, ops-drill
- **Where:**
  - `operator-ui/apps/fleet-manager/src/shared/api/hooks/use-authorization-watch/useAuthorizationWatch.ts:7,37`
  - mounted by `operator-ui/apps/fleet-manager/src/pages/authorization/AuthorizationPage.tsx:16`
    and `.../features/setup/components/setup-authorization/SetupAuthorization.tsx:15`

**What happens:** The watch polls on a flat 3-second interval with no decay and no ceiling. Each
tick issues two admin calls — `RefreshHolderAuthorizations` and `Onboarding`. Each refresh
notifies a daemon loop that performs one relay REQ, a retained-store read, and up to 64
signature verifications; the collector breaks on EOSE, so a 3-second caller is not throttled by
coalescing. The only stop condition is becoming authorized — which an unauthorized fleet, by
definition, never reaches.

**The result:** A visible, unauthorized Authorization tab costs roughly 1,200 relay REQs, 1,200
store reads and 2,400 admin calls per hour, against one relay, for as long as it stays on
screen. Hidden tabs do pause (the interval is gated on document visibility), so this is the cost
of a tab left *on screen* rather than merely open — but the page's own copy invites exactly
that: "This page stays available for as long as the fleet runs". The same module caps per-seat
fee polling at one call per 10 minutes and decays the boot query to 60 s.

**Failed defense:** The hook's comment defends the shared-cache-key hazard ("the worst case is
one extra relay read") — verified true, and irrelevant to the charge, which is the cadence.

**Fix direction:** Use the decay mechanism the module already owns — `pollIntervalMs` with a
3 s base and a 60 s ceiling — plus an explicit "Check now" control for the operator who is
actively waiting for authorization.

**What landed:** `authorizationPollMs` in the watch hook now drives `pollIntervalMs` with
`{ baseMs: 3_000, healthyMs: 3_000, ceilingMs: 60_000 }`, counting an unauthorized answer
towards the streak alongside a failure — an answer of "still nothing" costs the relay what an
error does. The cadence runs 3 s, 6 s, 12 s, 24 s, 48 s, 60 s and holds there; an observed
authorization still stops it outright. Both surfaces gained a secondary "Check now" button
(shared `Button`, existing `uActionsRow`) that forces a pass, so the decay never makes an
operator who has just had a holder sign wait out a minute.

Tests: `use-authorization-watch/__tests__/useAuthorizationWatch.test.ts` asserts the growth and
the ceiling; `AuthorizationPage.test.tsx` and `SetupAuthorization.test.tsx` each assert the
click forces a `RefreshHolderAuthorizations` + `Onboarding` pass, under fake timers that never
advance, so no automatic tick can stand in for the click. Falsified: restoring the flat 3 s
reddens the growth case (`expected 3000 to be 6000`); removing the control, and separately
leaving it in place but inert, redden both click cases.

**Not done here:** the shared `ONBOARDING_KEY` coupling is untouched. Decaying the cadence does
not decouple it — the mount/unmount flap `useOnboarding` documents is what holds the key
together — so splitting it stays a separate decision.
