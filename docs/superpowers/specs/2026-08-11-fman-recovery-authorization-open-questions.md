# FMan recovery and authorization: question status

Date: 2026-08-11
Reconciled: 2026-08-12
Source: [2026-08-11-fman-recovery-authorization-design.md](./2026-08-11-fman-recovery-authorization-design.md)

This record compares the recorded answers with the merged code. Merged code at
`origin/master` (`abb62855`) is the implementation source.

## Summary

| Question | Status | Result |
|---|---|---|
| Q1 | Answered and implemented | Recovery does not restore the price. |
| Q2 | Answered, not fully implemented | The daemon has both auth modes. Package wiring and pre-identity HTTP are missing. |
| Q3 | Answered, code contradicts it | Missing price or Holder authorization means setup is incomplete. The UI can finish without both. |
| Q4 | Not answered | Ask the direct wrong-phrase risk question. |
| Q5 | Not answered | Ask the direct lost-response risk question. |
| Q6 | Answered, partly implemented | Promote one untyped case, then carry codes on the wire. |
| Q7 | Partly answered and partly implemented | Durable retention is merged. Refresh result state and a startup fetch are missing. |
| Q8 | Waiting for credential-app | The dashboard has a payload. The credential app rejects it. |
| Q9 | Answered by code | Canonical mock keys work in `mocks/world/keys.ts`. |
| Q10 | Project task | Create and link tracker items. Do not treat this as a product question. |

## Q1 — recovery price

**Answer:** Do not restore the offer price. Use the minimum restore operation.
The operator can select the price again.

**Code status:** Implemented.

- `SeatBackupDocument` has no offer field.
- Restore writes seats and payments only.
- A new or restored FMan starts with a null price.
- Restore creates a new offer epoch. Old quotes are refused.

No open question remains. One UI residual: no screen states that the new offer
epoch voids quotes the original host issued.

## Q2 — browser authentication before identity setup

**Answer:** Umbrel authenticates the operator through the platform. StartOS
creates and shows a password during installation. This password does not depend
on the FMan identity.

**Code status:** Partly implemented.

- `AdminHttpAuth::TrustedProxy` supports the Umbrel model.
- `AdminHttpAuth::Password` supports the StartOS model.
- Password mode reads an owner-only password file.
- `crates/fman/specs/SPEC-operator-http.md:39-50` already specifies both
  deployments, so no design question remains.
- The session cookie is 32 random bytes made per process
  (`crates/operator-ui-auth/src/lib.rs:26`) in a process that spans both phases,
  so it survives the transition with no new mechanism.
- `admin_http` still starts only after identity setup:
  `Onboarding::run` at `crates/fman/bin/src/main.rs:490` runs before
  `admin_http::serve` at `:640`.
- No Umbrel Compose file and no StartOS manifest exist in this repository.

Required work:

1. Start the authenticated HTTP API during the onboarding phase, and swap the
   router at the phase change.
2. Wire Umbrel to trusted-proxy mode, in the packaging repository.
3. Wire StartOS to a generated installation password, in the packaging
   repository.

## Q3 — setup completion

**Answer:** Setup is incomplete when the price is absent. Setup is also
incomplete when Holder authorization is absent.

**Code status:** The merged UI contradicts this answer.

- `SetupAuthorization` permits **Skip for now**.
- `SetupPrice` permits an empty price and says **Finish setup**.
- `SetupGate` closes from local React state.
- The daemon stores no setup-complete state.

One product detail remains:

> When setup is incomplete, must the wizard block dashboard access, or can the
> operator use the dashboard with a persistent incomplete-setup state?

A new durable status verb may not be needed. Both facts are already on the wire:
the price through `ShowPlans` (a null price is a fleet that is not selling —
`crates/fman/core/migrations/0001_initial.sql:158`), and the authorization
through `Onboarding.nostr.state`. The only gap is that `onboarding_json`
(`crates/fman/core/src/admin.rs:455-465`) carries no price, so the UI needs two
reads unless the price is added there.

## Q4 — wrong valid recovery phrase

**Answer:** Not answered. The original preview protocol question was too
technical.

Ask this direct question:

> A valid phrase for another empty FMan can install a zero-seat identity. The
> host then refuses another restore. Is this risk accepted for the first
> release, or must the operator preview and confirm the derived FMan identity
> before installation?

No preview verb exists in merged code.

## Q5 — lost restore response

**Answer:** Not answered. The original durable-operation question was too
technical.

Ask this direct question:

> Restore can succeed while the browser loses the response. The UI can see that
> an identity exists, but it cannot recover the restore counts or prove which
> attempt completed. Is this risk accepted for the first release, or must
> restore use a durable operation ID and a status read?

One half of the original question is not a product question at all. Master has
recorded `crashed-restore-blocks-every-retry` as a found bug since 2026-08-09,
and its only remedy today is deleting a seat directory by hand — which no Umbrel
or StartOS operator can do. Open PR #327 adds restore ownership markers and fixes
it. Those markers do not add an operation ID and do not recover a lost browser
response.

## Q6 — recovery errors

**Answer:** Fix the protocol. Prefer typed Rust errors.

**Code status:** Partly implemented. Less work remains than this record first
stated.

`RestoreError` is already a typed enum with five named cases
(`crates/fman/core/src/restore.rs:77-98`). Two gaps remain. The admin envelope is
`Result<Value, String>` (`crates/fman/core/src/admin_http.rs:136`,
`admin.rs:210`), so the type is flattened to prose on the wire. And invalid
mnemonic is not a variant at all: it is
`anyhow::anyhow!("that is not a valid mnemonic phrase")` at
`crates/fman/core/src/onboarding.rs:141`.

So the work is to promote that one case, then carry a stable code beside the
existing operator message. The first code set must cover:

- invalid mnemonic;
- unreadable backup version;
- existing seat directory;
- missing guardian archive;
- already-onboarded host;
- relay failure.

This is an implementation task. It is not an open product question.

## Q7 — authorization state and retention

**Answer:** Retain the information that the UI needs. Cache Holder
authorizations when this protects the FMan's interests.

**Code status:** Partly implemented.

Merged code now verifies, merges, stores, reloads, and reuses Holder
authorization events. This answers the retention part.

The UI state part is still missing. `RefreshHolderAuthorizations` only schedules
work. `Onboarding` reports only `waiting_for_authorization` or
`authorization_observed` (`crates/fman/core/src/admin.rs:442`). A relay failure is
not observable through this API.

A second gap belongs here and was not in the original question. Nothing asks the
daemon to read the relay. `run_holder_authorization_refreshes`
(`crates/fman/nostr/src/lib.rs:330`) opens with `notified().await`, so there is no
fetch at startup, and PR #323 removed the 15-second poll. The only callers of
`RefreshHolderAuthorizations` are the setup authorization step and
`/authorization`. A restored host starts with an empty
`holder_authorization_events` table, because the backup document carries seats and
payments only (`crates/fman/core/src/backup.rs:78`), so it stays
`waiting_for_authorization` until a person opens one of those two screens. Under
the Q3 answer, that host reads as incomplete for as long as that lasts.

Required work:

- fetch once when the Nostr boundary starts, then continue on notify;
- report checking or scheduled state;
- report a completed empty check;
- report the last successful check time if the UI needs it.

Already satisfied, so cover it with a test rather than a change: a relay failure
does not delete a retained authorization. The empty arm is a no-op and every
error path continues without touching the presence watch
(`crates/fman/nostr/src/lib.rs:337` onward).

## Q8 — Holder authorization payload

**Answer:** No final decision *at the time of asking*. The holder side has since
been built: `credential-app` PR #132, `feat(holder): add app authorization flow`
(open and deployed to a Vercel preview).

**Code status:** The two sides already agree. The payload is decided in practice.

- The credential SDK defines `HolderAuthorizationRequest` with `subject_pubkey`.
- The FMan dashboard puts `{"subject_pubkey":"<64 hex>"}` in the QR.
- PR #132 accepts exactly that: `parseHolderAuthorizationRequest`
  (`src/credential/domain/holderAuthorization.ts`) is a Zod schema requiring a
  64-hex `subject_pubkey`, lowercased on the way in. Its own decisions record
  states "PeerBadge accepts the minimal SDK request" and "the same request works
  for FMan and FLIP".
- The authorization flow has its **own** parser. It does not go through
  `parseQrPayload`, the `type`-discriminated union
  (`fv:issuer-offer`, `fv:holder-request`, `fv:issuance-response`,
  `fv:credential`) used for badge and issuance QRs. An earlier revision of this
  record concluded from that union that the dashboard payload was rejected. That
  was wrong: it read the badge parser, on a checkout 46 commits stale.
- PR #132 also confirms Nostr kind `37705` and the current FMan tags, which is
  what `fetch_holder_authorizations` filters on.

What is left is not the payload shape but its forward compatibility. The schema
is `.strict()`, so **any** added key — a `type`, a `version`, an environment tag
— makes the holder application reject the whole request. So there is no room to
version the transport later without changing both sides in the same release. If
BE-FMAN-AUTH-002 wants a version or an environment binding, it has to be agreed
now, while there is one consumer.

Remaining work: one shared fixture across the SDK, the dashboard and
`credential-app`, so neither side can drift; and land PR #132.

## Q9 — canonical mock keys

**Answer:** Resolved by code.

Canonical keys live in `mocks/world/keys.ts`. Scenarios and tests import them,
and a test fails if an `npub` literal returns to the swept files. The lint and
boundary checks passed when the work landed.

No open question remains.

## Q10 — tracker ownership

**Answer:** This is not a product question.

No GitHub issue contains a `BE-FMAN-*` ID. The project owner must create and
link the tracker items. The browser flow must be tested with a real daemon after
the backend work lands.
