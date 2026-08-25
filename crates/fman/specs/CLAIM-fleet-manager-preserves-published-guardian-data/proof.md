# Proof: Published guardian data is not deleted

**Stale proof:** the source inventory has not been checked since the
setup-payment-policy rework and durable invite caching changed its scope and
exit paths. The Lean model also omits a wipe event when explicit restart's
`remove_dir_all` returns an error, even though recursive removal may already
have deleted part of the directory. The claim remains Unverified, and the Lean
theorem does not currently transfer to every Rust deletion execution.

## Property and scope

This implementation-grounded argument covers one seat under arbitrary FI
requests, crashes and restarts, child crashes and respawns, concurrent seat
verbs, and caller cancellation. The property begins when an invite reaches a
successful FI response, not when DKG first creates non-rederivable guardian
keys.

## Implementation argument

The source inventory identifies two guardian-directory deletion sites:

- the supervisor's optional pre-first-spawn wipe; and
- `SeatLoop::restart_dkg`'s explicit restart wipe.

It identifies two FI response surfaces that carry invites: `GetInviteCode` and
the running report used by `GetStatus`. On a fresh live observation, both paths
first commit the lifetime consensus observation and durable invite, update the
seat loop's mirrors, and only then expose the invite. Current code can also
answer `GetInviteCode` directly from the durable invite and can put that invite
in a cached unavailable `GetStatus` report without a fresh probe. Caller
cancellation does not cancel a command already owned by the seat loop.

One seat loop serializes probes, observation updates, and restart. A restart
checks the lifetime mirror before mutation. If its probe observes consensus, it
records that observation and refuses the restart. No invite command can
interleave between that decision and the explicit wipe. On startup, a
pre-first-spawn wipe is requested only for a latest durable attempt without a
guardian code. The supervisor consumes this flag before its first launch and
never restores it during respawn. Under the `fedimintd` lifecycle assumption, a
codeless current attempt cannot coexist with a current or prior consensus API
from which an invite could be emitted.

Both deletion sites derive the target from the typed seat ID. Database
uniqueness keeps sibling seat IDs distinct. Decommission and shutdown stop a
child but do not remove its directory.

This argument is incomplete at the current revision. The setup-payment-policy
rework touched the inventory's broad source scope, and durable invite caching
added replay paths that the source argument and Lean model do not cover. No
specifically requested verification re-established the current deletion and
invite inventories or the safety ordering of every current exit.

## Lean evidence

`lean/FMan/GuardianDataLoss/` models one seat's durable attempts, lifetime
consensus observation, volatile observation mirror, one-shot startup wipe flag,
child state, and ordered invite/wipe event log. Actor commands are effect plans,
and commands cannot interleave. The transition relation provides five named
crash prefixes that represent selected effect boundaries; it does not generate
every possible prefix mechanically. Probe results are adversarial. The
`FedimintdA2` hypothesis requires a consensus probe, including one attributed to
a reaped prior attempt, to have a spawned child and a coded current attempt.

`FMan.GuardianDataLoss.no_wipe_after_invite` proves that every reachable,
`FedimintdA2`-admissible model trace has no wipe after an invite.
`no_invite_then_wipe` states the corresponding transition property.
`FMan/Audit.lean` checks their axiom dependencies and rejects `sorryAx`;
`Counterexamples.lean` exhibits reachable violations when the restart lifetime
check moves after append, the supervisor keeps its wipe flag after first spawn,
or invite emission moves before durable observation.

The proposed correspondence maps:

- initial and restart attempt insertion to `appendCodeless`;
- guardian-code persistence to `recordGuardianCode`;
- durable and mirrored consensus observation to `recordConsensus` and
  `mirrorConsensus`;
- startup's codeless-attempt test and supervisor flag to `State.reboot` and
  `setWipeFirst`;
- child spawn and reap to `setChildRunning`;
- successful startup and restart removal to `emit .wipe`; and
- both invite exits to the record-before-`emit .invite` plan.

The model distinguishes non-rebooting invite-fetch failure, explicit-wipe
failure, and child-start failure after the startup wipe from process crashes.
This preserves their durable and volatile state more accurately than treating a
task-local error as a reboot.

Three correspondence gaps prevent transfer to the Rust property. First,
`Step.restartWipeFailed` emits no `.wipe`; a failed recursive removal is
therefore absent from the model log even when it partially deletes guardian
data. Second, the model has no durable invite field or transition for replaying
that invite while the child is unreachable; its invite transition requires a
running child. Third, the observational relation was argued only at points where
no seat-loop command is in progress. Its five crash constructors collapse other
await and statement boundaries by asserting that they add no modeled durable
fact or event, but that collapse is a hand-checked argument rather than a
construction. The relation otherwise requires agreement on durable attempts and
consensus, the volatile mirror, wipe flag and child state, and ordered inclusion
of Rust invite/wipe events in the model log.

The theorem remains useful as regression pressure on the modeled orderings, but
it proves only the transition system until those correspondence gaps and the
source inventory are checked.

## Residuals

- Before Fleet Manager returns an invite, DKG may already have written guardian
  keys. A pre-invite `RestartDKG` can delete them; this claim deliberately does
  not cover that window.
- Host-operator deletion, database corruption, alternate writers, memory
  corruption, and a self-destructive or lifecycle-violating `fedimintd` are
  excluded by the assumptions.
- The one-seat Lean model does not establish path construction or seat-ID
  uniqueness; those remain source-level obligations.

## Weakest links

The weakest links are the stale deletion-site and invite-exit inventories, the
unmodeled durable-invite replay, the missing failed-removal event, and the
hand-checked collapse of unmodeled crash boundaries. Next are the `fedimintd`
lifecycle contract, quiescent-point correspondence, and one-seat path-isolation
argument. The Lean theorem does not reduce those obligations.

## Destructive-restart boundary

A child may durably write non-rederivable guardian configuration before Fleet Manager observes consensus or an invite. Restart safety must consult that durable child/configuration state rather than only Fleet Manager's observation; the Lean model and correspondence cover this pre-observation wipe boundary.
