# FMan setup wizard: UI recovery, authorization access, and backend requests

Date: 2026-08-11
Scope: `operator-ui/apps/fleet-manager` and its mock surfaces only. No daemon change in this phase.

Delivery boundary: this phase unblocks UI work and mock-based review. It does not
make browser setup work against a real unconfigured FMan. That requires
`BE-FMAN-SETUP-001` at the end of this record. After the backend requests land,
the team must review this flow again before it treats the setup journey as
production-ready.

## Why

Three problems were raised against the FMan operator dashboard setup wizard.

1. After a recovery, the wizard decides where to send the operator. It decides
   from a value the daemon cannot supply at that moment.
2. The authorization step offers "Skip for now". After a skip, the QR code is
   unreachable. The wizard runs one time and does not open again.
3. The mock control panel names the wrong screen while the wizard is showing.

The review found three more problems.

4. The recovery screen shows no success state and only a small failure message.
5. The authorization screen text promises behaviour the screen does not have.
6. The real browser API starts after onboarding, so the browser cannot reach
   the setup wizard on an unconfigured host.

## Findings

### F0 — the real browser setup path is not reachable

The daemon serves onboarding through `admin.sock`, then opens the fleet, then
starts `admin_http` ([main.rs:430](../../../crates/fman/bin/src/main.rs),
[main.rs:561](../../../crates/fman/bin/src/main.rs)). A browser on an
unconfigured host therefore receives a network failure. It cannot call the
onboarding verbs that the setup wizard needs.

The UI and mock work in this record remains useful. It lets the team review and
test the interaction before the daemon work lands. It cannot close this
production gap.

### F1 — the price is not recovered

`SeatBackupDocument` ([backup.rs:78](../../../crates/fman/core/src/backup.rs)) carries
seats, payments, guardian archives, and decommission markers. It carries no offer
state. `install` ([restore.rs:164](../../../crates/fman/core/src/restore.rs)) writes
seats and payments only.

The database schema states the result directly
([0001_initial.sql:158](../../../crates/fman/core/migrations/0001_initial.sql)):

> A NULL price is a fleet that is not selling, which is where every fresh or
> restored FMan starts.

So a recovered FMan must set its price again. Existing seats keep the price
recorded in their own `quote_terms`. The `offer_epoch` is new, so quotes issued
by the original host are refused.

### F2 — the authorization survives, but cannot be read at once

`fetch_holder_authorizations` ([nostr/lib.rs:405](../../../crates/fman/nostr/src/lib.rs))
queries the relay by the FMan's own public key and uses no local data. That key
derives from the recovery phrase. So a holder authorization published for the
original host is still readable by the recovered host.

It is not readable immediately. `FleetManagerNostr::new`
([nostr/lib.rs:163](../../../crates/fman/nostr/src/lib.rs)) seeds the presence
channel with `WaitingForAuthorization`. Only `run_onboarding`
([nostr/lib.rs:309](../../../crates/fman/nostr/src/lib.rs)) corrects it, after a
relay connect and one fetch. `CONNECT_RETRY_INTERVAL` and
`ONBOARDING_POLL_INTERVAL` are both 15 seconds.

### F3 — the daemon has one answer for three situations

`Onboarding` ([admin.rs:312](../../../crates/fman/core/src/admin.rs)) reports
`waiting_for_authorization` in all three of these cases:

| Case | Truth |
|---|---|
| A | A holder authorized this FMan. The daemon has not read the relay yet. |
| B | No holder ever authorized this FMan. The operator skipped the step. |
| C | The daemon has not read the relay yet, and no authorization exists. |

Case B is common, because the wizard offers a skip.

### F4 — the wizard routes on that unreadable value

`handleRestored` ([SetupWizard.tsx:30](../../../operator-ui/apps/fleet-manager/src/features/setup/components/setup-wizard/SetupWizard.tsx))
refetches `Onboarding` immediately after the recovery and passes the result to
`onRestored` ([useSetupWizard.ts:44](../../../operator-ui/apps/fleet-manager/src/features/setup/hooks/use-setup-wizard/useSetupWizard.ts)),
which selects the price step or the authorization step.

By F2 and F3 that refetch returns `waiting_for_authorization` in almost every
real run. The price branch is code that a real daemon does not reach. Two unit
tests encode it as correct behaviour.

### F5 — there is no authorization surface outside the wizard

`NAV_ITEMS` ([nav-config.ts](../../../operator-ui/apps/fleet-manager/src/app/components/navigation-items/nav-config.ts))
holds Overview, Seats, Wallet, Backup. `deriveOverview`
([deriveOverview.ts](../../../operator-ui/apps/fleet-manager/src/features/overview/utils/deriveOverview.ts))
raises no item for an unauthorized FMan. The Backup page shows the service Nostr
pubkey but no authorization state and no QR code.

### F6 — the mock panel reads the wrong source

`MockPanelMount` ([MockPanelMount.tsx](../../../operator-ui/apps/fleet-manager/src/app/components/mock-panel-mount/MockPanelMount.tsx))
derives the screen name from `location.pathname`. Four surfaces are rendered by
gates and have no path of their own: the setup wizard, the sign-in prompt, the
daemon-error screen, and the boot screen. Each inherits the last path, usually
`/`, and the panel reports "Overview".

A second defect follows from the same cause. `scenarios.ts` tags `not-onboarded`
and `awaiting-authorization` with `affects: ['setup']`, but
[routes.ts](../../../operator-ui/apps/fleet-manager/src/mocks/routes.ts) can
never return `setup`. Those two scenarios reach no page tab today.

### F7 — the recovery screen has no result states

`SetupRestore` calls `onRestored` on success and moves on at once. The operator
sees nothing. The daemon returns `{ onboarded, seats, formed }` and the UI
discards all three.

A failure renders one line of red text under the checkbox. The daemon's restore
errors are larger than that. They include a leftover seat directory that the
operator must delete, a backup document this build cannot read, and a formed
seat whose guardian archive is missing.

A valid phrase that belongs to a different FMan succeeds with `seats: 0`.
Nothing in the flow tells the operator that this happened.

### F8 — the authorization screen text and key format are not accurate

The screen says it "notices on its own — leave it open". It does poll:
`useAuthorizationWatch` refetches every 3 seconds until the authorization is
observed. It does not advance. `SetupAuthorization.tsx:47` only enables
"Continue" and removes "Skip for now". The operator must click.

The value renders as bare `<code>` with no copy control. `BackupPage` pairs the
same class of value with `truncateMiddle` and `CopyButton`.

The real dashboard response is not an `npub`. `admin.rs` serializes
`nostr_sdk::PublicKey` with `to_string()`, which returns a 64-character
hexadecimal string. The mocks use an `npub`, so they do not match the real wire
value.

The screen also says that a holder scans the QR with their app. The holder app
flow and app-link format do not exist yet. The UI may show the raw service
Nostr public key, but it must not claim that scanning it completes the holder
flow.

## Decisions

| Topic | Decision |
|---|---|
| Delivery phase | Build and test the UI against mocks. Keep the real setup path blocked on `BE-FMAN-SETUP-001`. |
| Recovery result | Success state with exact seat-record counts. "Continue" only. Zero seats called out without guessing the cause. |
| Recovery failure | Full screen. Separate daemon refusal, authentication refusal, and an unknown network result. |
| Recovery journey | Always to the authorization step. Never assume an authorization exists. |
| Recovery phrase | Disable browser text services and remove the phrase from mutation state after settlement. |
| Authorization key | Full service Nostr public key with a copy button. No `npub` claim and no truncation. |
| Authorization advance | Automatic, with a status line, a spinner, and a manual control. |
| Authorization access | Permanent item in the main navigation. Careful status text on the Overview. |
| Mock panel | Each gate owns its own surface value. The store resolves the active value by priority. |
| Debug coverage | The panel can select every stable recovery and authorization result. Automated tests own short timer races. |
| Interrupted setup | Keep permanent recovery, authorization, and price signposts. Request durable setup progress from the backend. |

## Design

### D1 — recovery result states

`SetupRestore` owns a view state and selects from it. The mutation carries the
request only. It does not select the screen, because the component resets it
after settlement and an idle mutation can select nothing.

```ts
type RestoreViewState =
  | { type: 'form' }
  | { type: 'success'; result: SafeRestoreResult }
  | { type: 'failed'; error: SafeRestoreError }
  | { type: 'unknown'; error: SafeRestoreError };
```

`SafeRestoreResult` holds the daemon counts. `SafeRestoreError` holds the error
class and its message. Neither holds the phrase.

The result states are sibling components, one per file, because the project
allows one React unit per file.

```
setup-restore/SetupRestore.tsx          form, view state, and the selection
setup-restore-success/SetupRestoreSuccess.tsx   counts   [Continue]
setup-restore-failed/SetupRestoreFailed.tsx     message  [Try again] [Back to setup options]
setup-restore-unknown/SetupRestoreUnknown.tsx   status   [Check status]
```

The success state states the counts returned by the daemon. It labels `seats` as
seat records and `formed` as records that include guardian configuration. It
does not call either count active or running. The daemon response does not
support those claims.

When `seats` is 0, the screen states that no seat records were found. It lists
the safe possibilities: an empty fleet, missing relay records, the wrong
environment, or another valid phrase. It also states that the daemon has
installed the identity and that the operator cannot repeat setup on this host.
It offers "Continue" only. A "Back" control would suggest an undo that does not
exist.

The failure state replaces the screen. It shows the daemon's message unchanged
and does not classify it. The daemon's restore errors already name the cause and
the action; classifying them in the dashboard means matching prose, which the
project already treats as a defect (see `BE-FMAN-RECOVERY-003`).

For an `AdminApiError`, "Try again" returns to the form with the phrase still in
the field. A typo is a common cause, and 12 words are easy to enter incorrectly.
"Back to setup options" returns to the two doors and clears the visible field.
The daemon returns this error before it installs the identity.

An `AuthError` means that the authentication middleware refused the request
before dispatch. The sign-in gate does not open by itself. `useBootStatus` reads
authentication errors from the `Onboarding` query only
([useBootStatus.ts:33](../../../operator-ui/apps/fleet-manager/src/features/boot/hooks/use-boot-status/useBootStatus.ts)),
so a mutation error cannot open it. Without an added step the gate stays shut
until the query polls again, which is up to 60 seconds.

The mutation therefore refreshes that query and waits for the result:

```ts
await queryClient.refetchQueries({ queryKey: ONBOARDING_KEY, exact: true });
```

An awaited refetch, not an invalidation. Invalidation can also refetch an active
query, but this path needs an immediate and deterministic state change that a
test can assert. The UI does not keep the phrase across the gate.

A `NetworkError` has an unknown result. The daemon may have installed the
identity before the browser lost the response. This state does not offer another
restore. It offers "Check status". If `Onboarding` succeeds, the UI states that
the identity exists but that the recovery counts are unavailable, then continues
to authorization. If `Onboarding` returns the not-onboarded refusal, the UI
returns to the form. If the check cannot connect, the existing BootGate shows
the daemon-unavailable screen. After reconnection, a host with an identity
opens the app and its setup signposts. A host without an identity opens a new
setup wizard at the doors. `BE-FMAN-RECOVERY-002` replaces this inference with
an explicit operation result and exact counts.

`SetupRestore` copies the safe response or error into its view state, then
resets the mutation. The restore mutation uses `gcTime: 0`. This prevents
TanStack Query from keeping the recovery phrase in its mutation variables after
settlement. The textarea sets `autoComplete="off"`, `autoCapitalize="none"`,
`autoCorrect="off"`, and `spellCheck={false}`.

### D2 — recovery journey

`onRestored` loses its boolean and `handleRestored` loses its refetch. Recovery
always reaches the authorization step.

```
doors ─▶ recovery form ─▶ recovery success ─▶ authorization ─▶ price ─▶ overview
                       └▶ recovery failed
```

Case A of F3 continues by itself within about one daemon cycle. Case B stays on
the QR, which is correct. Case C becomes case A or case B when the daemon reads
the relay. No decision is made from an unreadable value.

The authorization step carries no recovery text. The counts belong to the
recovery result, which is where D1 puts them.

Both identity-creation mutations reset `Onboarding` query data after success and
wait for one fresh response. `SetupAuthorization` does not auto-continue from a
cached authorization that belonged to an earlier daemon identity. A regression
test seeds the cache with an authorized identity, restores another identity,
and proves that only the fresh response controls the next step.

### D3 — shared authorization surface

`features` may not import each other, so the shared parts move to `shared`.

```
shared/utils/authorization.ts               buildAuthorizationPayload, isAuthorized
shared/api/hooks/use-authorization-watch/   useAuthorizationWatch
shared/components/authorization-panel/      QR, full Nostr public key + CopyButton, status banner
```

`AuthorizationPanel` renders no actions. Each surface owns its own.
`isNotOnboardedError` stays in `features/setup/utils`, because only `SetupGate`
uses it.

The service Nostr public key renders in full, in `uLongIdText`, with a
`CopyButton` beside it. The UI treats the current wire value as hexadecimal.
This differs from `BackupPage` on purpose. That page lists keys for reference,
where middle truncation is right. This screen shows one value that a holder may
compare with their own application. A truncated value does not permit that
check. A comment records the reason.

#### The key-format sweep

Every current fixture uses an `npub`, which no daemon response can produce. The
correction is repository-wide, not one file. `npub` literals exist in the mock
world and in the setup, boot, authorization, overview, and backup tests. The
sweep covers every `service_nostr_pubkey` value and every `holders` entry.

One module owns the canonical values, so the mock world and the tests cannot
drift apart again:

```
mocks/world/keys.ts    MOCK_SERVICE_NOSTR_PUBKEY, MOCK_HOLDER_PUBKEY
                       64 lowercase hexadecimal characters each
```

`scenarios.ts` imports them. Unit tests import them. The boundaries rule
classifies only `shared`, `feature` and `app`, at `warn` severity, so an
unclassified dependency does not fail `element-types`. If it warns in practice,
the constants move to a test-support module and `scenarios.ts` keeps its own
copy with a comment linking the two.

The UI never parses this value. It copies it, renders it, and encodes it in the
QR. So a realistic 64-character string is enough; a valid curve point is not
required.

The panel says that a holder or holder tool can use the key. It does not say
that a holder app can scan and complete the flow. `BE-FMAN-AUTH-002` owns the
future QR payload contract.

Before the first `Onboarding` response, the panel shows an explicit loading
state. It does not render waiting text from missing data. An error with no data
shows the error state. An error with cached data keeps the last known key and
status visible and adds a refresh warning.

### D4 — authorization in the main navigation

`NAV_ITEMS` gains `{ key: 'authorization', label: 'Authorization', path: '/authorization' }`,
placed after Overview, always present.

`pages/authorization/AuthorizationPage.tsx` wraps `AuthorizationPanel` and adds
the observed holder list. It has no skip, no continue, and no redirect.

`deriveOverview` gains an attention item, "Authorization has not been observed",
linked to `/authorization`, when `nostr.state` is
`waiting_for_authorization`. Its detail explains that the daemon may still be
checking the relay. The UI does not state that the fleet has no authorization.
`BE-FMAN-AUTH-001` owns the daemon state needed for a definitive message. This
uses the attention list that already exists. No navigation badge is added.

### D5 — automatic continue

When the authorization is observed, `SetupAuthorization` shows:

```
✓  Authorization observed.
⟳  Continuing to the price step…              [ Continue now ]
```

- The status line uses `role="status"`, so a screen reader announces it.
- The wait is about 2 seconds, then `onSettled()` runs.
- "Skip for now" is removed. The button row keeps its position, so "Continue"
  does not move under the pointer.
- "Continue now" stays active for an operator who does not want to wait.
- Under `prefers-reduced-motion` the spinner does not turn. The text carries the
  message.
- The timer is cleared on unmount.
- One shared guard protects both the timer and the manual button. `onSettled()`
  runs once when both actions occur in the same event window.

The standalone page shows the same success message and the holder list, with no
spinner and no redirect.

There is no shared `Spinner`. The only one lives inside `Button`
([Button.module.css:33](../../../operator-ui/packages/shared-ui/src/components/button/Button.module.css)).
This screen gets a local one. The project extracts on the third use; this is the
second.

### D6 — polling stops on skip

"Skip for now" unmounts the authorization screen, which unmounts the 3-second
watch observer. The 60-second observers in the gates remain. So the fast poll
stops, but `Onboarding` calls do not stop completely.

Nothing states this. It follows from where the hook sits. A fake-timer test moves
past one 3-second interval and stays below 60 seconds. It asserts that the fast
observer makes no call after a skip. A later change that hoists the hook cannot
pass this test.

### D7 — mock panel gate surface

A small store in `shared/surface/` records one value for each gate owner. Each
gate updates and clears only its own value.

```
BootGate   → 'boot' | 'auth' | 'daemon-error'
SetupGate  → 'setup'
```

The store resolves the active value in this order: a protected BootGate surface,
the SetupGate surface, then `location.pathname`. A parent cleanup cannot clear a
child owner's value. `MockPanelMount` uses the resolved value. The store lives in
`shared` and not in `mocks`, because the gates must not import `@/mocks/*`. That
rule keeps the mock world out of production bundles.

`routes.ts` gains an `authorization` key. `awaiting-authorization` is tagged with
it. The `affects: ['setup']` entries become reachable for the first time.

Tests cover the transitions from boot to setup, auth to setup, setup to a route,
and cleanup in React StrictMode.

### D8 — interruption signposts

The UI cannot know whether an operator recorded the recovery phrase. It cannot
restore the in-memory wizard step after a page reload. The production solution
needs `BE-FMAN-SETUP-002`.

This UI phase keeps every safe action reachable after a reload:

- Backup links to the recovery phrase and states that the browser did not save
  it.
- Authorization stays available from the main navigation.
- Overview shows the current offer through "Change price".

The UI does not claim that these links prove setup completion.

### D9 — setup price bounds

`parsePriceField` checks `Number.isSafeInteger(priceMsat)` after the sats-to-msats
conversion. It rejects values that JSON cannot represent exactly. The same
shared parser protects both setup and the existing Offer page.

### D10 — debug-panel state coverage

The debug panel must create each stable state added by this record. A tester
must not need the dotted-path editor for these paths.

`MockState` gains four typed controls:

| Control | Values |
|---|---|
| Restore result | `2 seats / 1 formed`, `2 seats / 0 formed`, `0 seats` |
| Restore transport | `normal`, `fail before dispatch`, `fail after commit` |
| Restore session | `active`, `expire on submit` |
| Onboarding transport | `normal`, `network failure` |

`expire on submit` applies only to the next `OnboardFromBackup` call. The handler
sets the mock session to inactive and returns HTTP 401 before dispatch. Changing
this control does not expire the current session, so the recovery form stays
open until the tester submits it.

The mock handler implements transport failures at the HTTP boundary:

- `fail before dispatch` returns a network failure and does not call
  `OnboardFromBackup`.
- `fail after commit` calls `OnboardFromBackup`, persists the changed world,
  then returns a network failure.
- `Onboarding transport: network failure` fails the later status check. It does
  not return `{ Err: ... }`.

The existing per-verb error control continues to return daemon `{ Err: ... }`
responses. Its recovery choices gain the real classes of message that the UI
must display: invalid mnemonic, unreadable backup version, existing seat
directory, and missing guardian archive.

The scenario catalog covers the stable authorization pages:

- `awaiting-authorization` shows the waiting panel and Overview signpost.
- `authorization-observed` shows the holder list and full hexadecimal service
  Nostr public key.
- `authorization-read-error` shows the read error while the app shell remains
  available.

`not-onboarded` remains the entry point for new setup and every recovery result.
The panel lists it under the `setup` surface after D7 lands. The three
authorization scenarios list `setup` and `authorization` in `affects`. A tester
can therefore move the wizard from waiting to observed without leaving the
setup tab. Relevant scenarios also list `overview` when they change its
signposts.

The panel and automated tests cover this matrix:

| State to inspect | Panel setup | Expected UI |
|---|---|---|
| Recovery request pending | Add mock latency, then submit | Disabled actions and visible progress |
| Recovery with seat records | `not-onboarded`; result `2 / 1`; normal transport | Success screen with exact labels |
| Recovery with no formed record | `not-onboarded`; result `2 / 0`; normal transport | Success screen with zero formed records |
| Recovery with no seat record | `not-onboarded`; result `0`; normal transport | Zero-seat warning and Continue only |
| Daemon recovery refusal | Force `OnboardFromBackup` error | Full daemon-error result with retry actions |
| Authentication refusal | Restore session `expire on submit` | Sign-in gate after submit; no retained phrase |
| Network failure before dispatch | Restore transport `fail before dispatch` | Unknown result; status check returns to form |
| Network failure after commit | Restore transport `fail after commit` | Unknown result; status check continues without counts |
| Status check cannot connect | Any network restore failure plus Onboarding network failure | Global daemon-unavailable gate; reconnect routes from daemon state |
| Authorization waiting | `awaiting-authorization` | Full key, copy control, waiting text |
| Authorization loading | Add mock latency before opening Authorization | Loading state without waiting text |
| Authorization observed | `authorization-observed` | Holder list and observed text |
| Authorization read error | `authorization-read-error` | Error text without a false waiting claim |
| Authorization refresh, daemon error | Open observed state, then force an `Onboarding` `{Err}` response | `AdminApiError`. Last known key and status, plus a refresh warning |
| Authorization refresh, transport error, cached | Open observed state, then set Onboarding transport to network failure | `NetworkError`. Last known key and status, plus a refresh warning |
| Authorization refresh, transport error, no cache | Set Onboarding transport to network failure before the first response | `NetworkError` with no data. The daemon-unavailable gate, not a waiting claim |

The automatic two-second transition is not a persistent debug-panel state. A
fake-timer component test covers the timer, manual-button race, cleanup, and
reduced-motion spinner. The standalone Authorization page provides a stable
observed state for visual review.

## Delivery sequence

The UI work is three plans, not one. The order is a dependency, not a preference.

| Plan | Content | Depends on |
|---|---|---|
| A | The key-format sweep (D3). The `parsePriceField` bound (D9). | — |
| B | Recovery result states (D1). Recovery journey (D2). Shared panel (D3). Navigation and Overview signpost (D4). Automatic continue (D5). Skip test (D6). Gate surface (D7). Signposts (D8). | A |
| C | Mock transport failures and the debug-control matrix (D10). | B |

Plan A goes first because it corrects fixtures that disagree with the daemon.
Every test written after it then rests on true values. It is small and it
touches no behaviour.

Plan C is test infrastructure, not operator-visible behaviour. It must not delay
Plan B.

Then:

1. Use the mock setup journey for product and accessibility review.
2. Ask the backend owner to create tracked work for each `BE-FMAN-*` request.
3. Link the backend issues to the request IDs at the end of this record.
4. Review the UI flow again after the backend contracts land.
5. Add real-daemon browser coverage before production release.

Plans A to C unblock frontend work. They do not remove the production block in
F0.

## Module boundaries

The import direction stays as the project defines it.

```
shared  ←  features  ←  pages/app
```

- `shared/components/authorization-panel` imports `shared` only.
- `features/setup` imports the panel from `shared`. It imports no other feature.
- `pages/authorization` imports the panel from `shared`.
- `app` gate components import the surface store from `shared`.

## Files

New:

```
shared/utils/authorization.ts                            (+ __tests__)
shared/api/hooks/use-authorization-watch/                (moved from features/setup)
shared/components/authorization-panel/AuthorizationPanel.tsx (+ module, __tests__)
shared/surface/gateSurface.ts                            (+ __tests__)
pages/authorization/AuthorizationPage.tsx                (+ module, __tests__)
features/setup/components/setup-restore-success/         (+ module, __tests__)
features/setup/components/setup-restore-failed/          (+ module, __tests__)
features/setup/components/setup-restore-unknown/         (+ module, __tests__)
mocks/world/keys.ts                                      canonical hexadecimal keys
```

Changed:

```
features/setup/components/setup-restore/SetupRestore.tsx
features/setup/api/hooks/use-onboard-from-backup/useOnboardFromBackup.ts
features/setup/api/hooks/use-onboard-as-new/useOnboardAsNew.ts
features/setup/components/setup-authorization/SetupAuthorization.tsx (+ module)
features/setup/components/setup-wizard/SetupWizard.tsx
features/setup/hooks/use-setup-wizard/useSetupWizard.ts
features/setup/utils/setupState.ts                       (authorization helpers move out)
features/overview/utils/deriveOverview.ts
pages/overview/OverviewPage.tsx                          (passes onboarding state)
pages/backup/BackupPage.tsx                              (reload signpost)
shared/utils/offerPrice.ts                               (safe integer bound)
app/components/navigation-items/nav-config.ts
app/components/boot-gate/BootGate.tsx
app/components/setup-gate/SetupGate.tsx
app/components/mock-panel-mount/MockPanelMount.tsx
app/index.tsx                                            (authorization route)
mocks/state.ts                                           (typed setup controls)
mocks/handlers.ts                                        (transport failure timing)
mocks/world/verbs.ts                                     (recovery result variants)
mocks/panel-config.ts                                    (setup and session controls)
mocks/routes.ts
mocks/scenarios.ts
```

Key-format sweep — every file below replaces an `npub` literal with the
canonical hexadecimal value:

```
mocks/scenarios.ts
features/boot/hooks/use-boot-status/__tests__/useBootStatus.test.tsx
features/setup/utils/__tests__/setupState.test.ts
features/setup/components/setup-wizard/__tests__/SetupWizard.test.tsx
features/setup/components/setup-authorization/__tests__/SetupAuthorization.test.tsx
pages/overview/__tests__/OverviewPage.test.tsx
pages/backup/__tests__/BackupPage.test.tsx
```

The implementer re-runs `grep -rn npub apps/fleet-manager/src` and expects no
match outside a comment that explains why the format is not `npub`.

## Tests

Existing tests that change:

- `SetupWizard` "should skip the QR when a restored fleet is already authorized"
  and "should stop a restored fleet at the QR when the relay still reports it
  waiting" collapse into one case: recovery always reaches the authorization step.
- `SetupWizard` "should complete setup once the price is stored" loses its
  "Continue" click on the authorization step.
- Both onboarding hooks reset cached `Onboarding` data after an identity change.
- `SetupAuthorization` "should confirm and allow continuing once the
  authorization is observed" becomes an automatic-continue assertion.
- `mocks/routes` gains the `authorization` key.

New coverage:

- `AuthorizationPanel` renders the full key and a copy control.
- `AuthorizationPanel`, the mock world, and every test fixture use the
  hexadecimal key format from one shared module.
- `AuthorizationPanel` distinguishes loading, no-data error, and cached-data
  refresh error.
- `SetupRestore` success state, including accurate zero-seat text.
- `SetupRestore` separates daemon refusal, authentication refusal, and an
  unknown network result.
- `SetupRestore` removes the phrase from mutation state after settlement.
- `SetupRestore` disables browser text services on the phrase field.
- `SetupRestore` selects its screen from its own view state, and still shows the
  result after the mutation is reset.
- A mutation `AuthError` refetches `ONBOARDING_KEY` and the sign-in gate opens
  without waiting for the 60-second poll.
- `SetupWizard` does not use cached authorization data from an earlier identity.
- `SetupAuthorization` continues once when the timer and manual action race.
- `SetupAuthorization` stops its 3-second poll after a skip (D6).
- `AuthorizationPage` renders waiting and observed states.
- `deriveOverview` states that authorization has not been observed. It does not
  state that no authorization exists.
- `gateSurface` covers gate priority, owner cleanup, and gate transitions.
- `parsePriceField` rejects a price that cannot be represented exactly.
- Mock verbs return each configured recovery count.
- Mock HTTP handlers fail before dispatch and after commit without confusing a
  transport failure with a daemon `{ Err: ... }` response.
- Debug-panel controls persist and reset with the selected scenario.
- Scenario filtering lists setup and authorization states on the correct panel
  tab.

## Out of scope

- Any daemon change. The requests below own that work.
- Production browser setup. `BE-FMAN-SETUP-001` blocks it.
- A complete holder-app scan flow. The UI keeps the bare service Nostr public
  key until `BE-FMAN-AUTH-002` defines the shared payload.
- Tuning the 3-second watch interval. The daemon reads the relay every 15
  seconds, so the dashboard asks about five times more often than a new answer
  can appear. This is existing behaviour and is left alone.
- A navigation badge for the authorization state. The Overview attention item
  covers the need with a mechanism that already exists.

## Backend requests

These IDs are stable references for backend planning. They are requests in this
record. No issue-tracker item exists until the backend owner creates one and
links it here.

### BE-FMAN-SETUP-001 — serve the browser API during onboarding

Start the authenticated browser admin API before the host has an identity. Keep
the listener, authentication mode, and session secret across the transition to
the running fleet.

Acceptance:

- `POST /api/admin` on an unconfigured host returns the onboarding refusal from
  `Onboarding`. It does not fail at the network layer.
- `OnboardAsNew` and `OnboardFromBackup` work through HTTP.
- The request receives its final response before the dispatcher changes phase.
- The same browser session can call `ShowMnemonic` and `Onboarding` after the
  phase change.
- A real-daemon browser test covers new setup and recovery setup.

Blocks: production use of every UI setup screen in this record.

### BE-FMAN-SETUP-002 — expose durable, non-secret setup progress

Add a read-only setup status and an explicit setup-complete action. Store no
mnemonic or browser data in this status. The UI must be able to choose a safe
next step after a reload or browser failure.

Acceptance:

- A reload after identity creation returns a resumable setup state.
- The state distinguishes an incomplete wizard from a host whose operator
  chose a blank price and skipped authorization.
- Completing setup is idempotent.
- The contract does not claim that the daemon can prove the operator wrote down
  the phrase.

Blocks: a reliable interrupted-setup journey.

### BE-FMAN-RECOVERY-001 — preview recovery before identity installation

Add a read-only recovery preview. It derives the candidate identity and reads
the relay records without writing the database or seat directories. Return the
candidate FMan name, service Nostr public key, seat-record count, formed-record
count, and decommissioned-record count.

**This request needs no new recovery logic.** The daemon already separates
reading from writing. `recover()` "reads nothing local and writes nothing at
all", and `RecoveredFleet` is "assembled whole and inspected before it touches
the disk, so an operator can be shown what is about to be recovered"
([restore.rs:54](../../../crates/fman/core/src/restore.rs)). The work is a verb
and a protocol around code that exists.

What it needs:

- a preview verb;
- a response type;
- a preview and confirmation protocol;
- protection against a changed or stale preview;
- tests.

Acceptance:

- A zero-seat preview changes no local state.
- The operator must confirm the preview before installation.
- Installation binds to the previewed identity and backup result. A changed
  preview cannot install without another confirmation.
- The response states the environment name and relay host name. It must remove
  relay user information and credentials.

Blocks: safe correction of a valid but wrong phrase and useful zero-seat
guidance.

### BE-FMAN-RECOVERY-002 — make an unknown restore result recoverable

Give each restore attempt a client-supplied operation ID. Let the UI read the
result after a lost response. Repeating the same operation ID must return the
same result and must not run installation again.

**This is the hardest of the seven requests.** A ledger alone does not make the
operation repeatable. Two facts about the current path decide the design.

First, the ledger must be durable **before** installation starts. A daemon
restart during installation otherwise loses the operation identity, and nothing
can reconcile the attempt. The pre-identity path writes nothing today. The
recovery result is only durable when `install_restored_fleet` commits at the end
of `install` ([restore.rs:211](../../../crates/fman/core/src/restore.rs)).

Second, `install` writes seat directories before that database commit
([restore.rs:188](../../../crates/fman/core/src/restore.rs)). A crash can
therefore leave filesystem data with no completed database record. The existing
`SeatDirectoryExists` error is the symptom of exactly this, and it currently
tells the operator to delete the directory by hand.

The request must define:

- creation of a durable pending operation before installation begins;
- stable operation-key handling;
- atomic success recording with the database installation where the storage
  permits it;
- restart handling for an operation still marked pending;
- reconciliation of seat directories created by an incomplete attempt;
- the retry result for each of completed, pending, and failed.

Acceptance:

- A response loss after installation can be reconciled.
- A retry cannot replace an installed identity.
- The status response contains the same seat counts as the first response.
- The operation result survives the onboarding phase change and a daemon
  restart.
- A restart during installation leaves a state the daemon can resolve without
  manual file deletion.
- Operation IDs contain no mnemonic material.

Blocks: safe handling of network failure during recovery.

### BE-FMAN-RECOVERY-003 — return structured recovery errors

Return a stable error code with the existing operator message. Cover invalid
mnemonic, unreadable backup version, existing seat directory, missing guardian
archive, already onboarded host, and relay failure.

Acceptance:

- The UI can select recovery actions without matching message text.
- The daemon keeps the detailed message for operators and logs.
- Tests bind each recovery failure branch to one stable code.

Blocks: cause-specific recovery help. It does not block the UI-only failure
screen.

### BE-FMAN-AUTH-001 — report authorization read state

Replace the two-state projection with states for checking, checked with no
authorization, authorization observed, and relay error. Include the last
successful check time when one exists.

Acceptance:

- Daemon startup reports `checking`, not "no authorization".
- A completed empty fetch reports `not_observed`.
- A failed fetch does not erase a previously observed authorization.
- The Overview can state the current result without guessing.

Blocks: definitive authorization warnings and useful relay-failure guidance.

### BE-FMAN-AUTH-002 — define the holder authorization payload

Define one payload with the holder-app owner. Specify the URI or app-link
scheme, public-key encoding, network or environment binding, and version. Keep
the raw service Nostr public key available for manual comparison.

Acceptance:

- The daemon, dashboard, and holder app share one versioned payload fixture.
- The holder app can scan the dashboard QR and show the same FMan identity.
- The payload cannot authorize a different environment by accident.
- The dashboard mocks use the shared fixture.

Blocks: UI text that tells the operator that a holder app can scan and complete
authorization.
