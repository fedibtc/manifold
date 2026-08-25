# Proof: A guardian identity runs in only one Fleet Manager instance



Scope: `crates/fman/core/src/**`, `crates/fman/core/tests/**`,
`crates/fman/core/migrations/**`, `crates/fman/bin/src/**`,
`crates/fman/specs/SPEC-nostr-backup-restore.md`

## Claim

Under A-operator, no two live Fleet Manager instances with different data roots
host the same formed guardian identity, despite arbitrary daemon crashes and
restarts, concurrent onboarding on the same or different hosts, child crashes
and respawns, and restore racing an old host.

An **instance** is identified by its data root. “Guardian identity” means the
whole formed guardian seat: its consensus key material is the immutable
archive produced by DKG, while its Iroh endpoint keys and API authentication
derive from the root mnemonic and seat id. The mnemonic locates and decrypts
the archive; it does not derive the consensus shares. Merely running an FMan
with no formed seat is not a live guardian.

This claim deliberately assigns cross-data-root mnemonic ownership to the
human operator. The daemon enforces the restore acknowledgement and local
empty-install guards; it does not implement a distributed per-identity lock.

## Axioms (trusted, not checked here)

- **A-valid-history:** each data root begins empty or was produced by this
  implementation. No non-daemon process edits an identity database or seat
  directory, starts another fedimintd from it, or bypasses the admin API.
  SQLite commits and constraints are durable and faithfully loaded.
- **A-key-generation:** fresh BIP-39 generation does not repeat an existing
  root mnemonic, and the documented mnemonic/seat derivation does not collide
  for distinct inputs. This covers the deterministic endpoint/authentication
  credentials and backup discovery keys, not fedimint's archived DKG shares.
- **A-operator:** the operator globally serializes one root mnemonic's
  ownership across data roots. They never supply, copy, or restore the same
  mnemonic into two instances whose daemon or guardian-child lifetimes can
  overlap; before acknowledging restore, they have permanently retired every
  prior instance and they do not concurrently restore another successor. This
  is the human constraint accepted by `SPEC-nostr-backup-restore`: the daemon
  requires the assertion but cannot observe or enforce its global truth.

## Argument

**L1 (enum) — every way a mnemonic enters production daemon state.** Searching
all `RootMnemonic::{generate,parse}` calls and both identity-row writers gives
exactly these production paths:

1. `OnboardAsNew` calls `onboard_as_new`, freshly generates a mnemonic, and
   writes it through `Db::install_identity`.
2. `OnboardFromBackup` parses the caller's mnemonic and eventually writes it
   through `Db::install_restored_fleet`.
3. Startup calls `Db::load_identity`, which parses the mnemonic already in
   this data root; it does not introduce one.

The other parse/generate calls are tests. `ShowMnemonic` only exports an
existing phrase. Service, wallet, backup, fee, and seat endpoint/authentication
keys are derived from the loaded root and do not install another root. The
formed guardian's consensus shares instead enter restore through the archive
which that root discovers and decrypts. Thus, under A-key-generation, path 2 is
the only daemon path that can intentionally reintroduce both an existing
archive-backed guardian and its deterministic seat credentials.

**L2 (code + test) — restore cannot install before the acknowledgement.**
`Onboarding::restore` first applies `ensure!(acknowledged)`, then parses the
mnemonic, recovers its documents, and calls `restore::install`; only that last
call can reach `Db::install_restored_fleet`. The refusal is pinned by
`restoring_needs_an_acknowledgement`, including that no identity row appears.
There is no await or write before the check. Consequently cancellation or a
crash cannot turn an unacknowledged request into installed state; an identity
transaction committed after the check remains startup-loadable even if the
process crashes before replying. The database does not record the flag; the
claim is about this causal trace ordering, under A-valid-history.

**L3 (code + test) — restore is adoption into an empty local install, not a
local overwrite.** `restore::install` refuses if `load_identity` returns a row,
then checks every recovered seat destination before creating any directory and
refuses if any exists. `install_restored_fleet` inserts the seats and identity
in one transaction, with the identity insert protected by its fixed primary
key. `an_install_that_has_been_onboarded_refuses_to_be_restored_into`,
`a_restore_never_writes_into_an_existing_seat_directory`, and
`a_second_onboarding_cannot_replace_the_first` pin these refusals. These guards
do not detect the same mnemonic on another data root; they establish only that
restore cannot manufacture a second local copy by reconciling into a running
install.

**L4 (enum) — every way a guardian child becomes live.** Regenerating all
`Seat::start`, `SeatSupervisor::spawn`, and `SeatProcess::start` call sites
yields:

- `Fleet::open_with_wallet` loads every stored seat and calls `Seat::start`;
- the successful new-seat insertion path calls `Seat::start` for that new
  seat;
- `restart_dkg` stops the existing supervisor and starts its replacement; and
- the supervisor loop calls `SeatProcess::start` initially and after a child
  exit.

Decommissioned seats do not start a loop or child. There is one official daemon
child-spawn site, inside `SeatProcess::start`. The binary can dispatch itself as
fedimintd when directly invoked under a `fedimintd` argv[0], but no second daemon
path invokes that mode; direct external invocation is excluded by
A-valid-history.

**L5 (code) — the data-root flock is not a child-lifetime lock.** `Db::open`
takes the flock before opening or migrating SQLite, and every `Db` clone retains
it. The binary carries one such handle continuously through onboarding into
`Fleet::open_with_wallet`; detached seat loops and the callback worker also own
database clones through their cleanup. Two daemon phases therefore cannot
concurrently own that same root, and ordinary startup failure after a seat loop
starts cannot release the flock ahead of that loop. The lock is explicitly not
an identity lock and says nothing about another root containing the same
mnemonic.

Even on one root, the code does not prove that guardian child lifetimes never
overlap. `restart_dkg` awaits `SeatSupervisor::stop`, but the supervisor logs and
swallows `SeatProcess::stop` errors. A `process.wait()` error also drops a
kill-on-drop child without proving it reaped before the supervisor retries. A
hard-killed daemon releases its flock when the parent exits; delivery of the
child's parent-death signal is not a reap-before-reacquire barrier. Successful
exit/stop paths attempt orderly replacement, but the error paths do not await
that postcondition. These overlap windows reuse an already-installed root and
do not install a mnemonic; they therefore cannot bypass L2's gate, but L5 supplies no broader local
no-two-live-children conclusion.

**L6 (enum + code) — the gate covers every daemon-created repeated identity.**
A fresh root cannot repeat under A-key-generation and cannot discover the old
root's encrypted archive. A newly sold seat uses the current root but a
database-unique seat id and obtains its consensus shares from its own DKG, so it
is a new guardian identity. Startup and supervisor/restart paths reuse only the already-installed local
identity; L5 records why their ordinary and error paths do not install it.
Under A-valid-history and A-operator, a second data root cannot acquire the
same stored identity by external copying. The remaining case is restore (L1),
whose identity install is causally after the explicit true acknowledgement (L2). Local identity and
directory refusals cannot weaken that ordering (L3).

**L7 (axiom) — mnemonic ownership transfers between data roots without
overlap.** By A-operator, the explicit acknowledgement used in L2 is a globally
serialized transfer: every previous instance is permanently offline, no other
successor restore is concurrent, and a retired root will not restart. This is
not derived from the boolean or the flock; it is exactly where the cross-host
guarantee bottoms out in a human premise.

**Conclusion.** Suppose two different data roots hosted the same formed
guardian simultaneously. Fresh onboarding cannot repeat the root under
A-key-generation, and external copying is outside A-valid-history/A-operator.
Therefore a
second root must have acquired it through restore (L1/L6), after the explicit
acknowledgement (L2). A-operator makes that acknowledgement a serialized
transfer from permanently retired prior roots (L7), contradicting simultaneous
liveness. The local database/directory guards ensure restore did not instead
reconcile into an existing instance (L3). The flock contributes only
same-root daemon exclusion and is not treated as a per-identity mechanism
(L5). ∎

## Residual windows (outside the claim; acceptance stated per window)

- **Operator violates global mnemonic ownership.** Restoring while an old or
  concurrent successor instance can run produces the feared duplicate. This
  is outside A-operator and is the explicitly accepted human boundary in
  `SPEC-nostr-backup-restore`: it is “the one constraint that cannot be moved
  off a human.”
- **Same-root child-shutdown overlap (unaccepted here; separately owned).**
  Supervisor stop/wait errors and a hard parent kill do not prove reap before
  replacement. These paths
  install no mnemonic and so
  occur within one instance as defined by its data root. They prevent deriving
  a child-process uniqueness claim from this record, but do not create a second
  FMan instance. A broader no-equivocation parent must keep this as a separate
  proof obligation unless the owner explicitly accepts it.
- **One in-flight-session equivocation window.** Even with only one live host,
  a guardian restored without its latest in-flight session state can conflict
  with its own earlier contribution to that session. Finalized sessions replay
  from threshold-signed history. `SPEC-nostr-backup-restore` explicitly
  accepts this boundary; it is state loss, not two concurrently live daemon
  instances, so it is outside this claim's bad thing.
- Operator copying and reuse are excluded by A-operator. Direct fedimintd
  launch, database corruption, and mnemonic or derivation collisions are
  excluded by A-valid-history and A-key-generation rather than prevented by the
  restore gate.

## Weakest links

Weakest links, in order: A-operator's global human coordination, then
L1/L4/L6's production-path enumerations, A-key-generation, and L2/L3's local
guards. The acknowledgement and restore-refusal tests are the strongest
regression rungs. Nothing upgrades the per-data-root flock to a per-identity
lock; cross-root uniqueness follows entirely from the operator axiom.

## Falsification procedure

Run cold from the claim and axioms, without relying on the argument above.

1. Enumerate every production call that generates, parses, loads, or writes a
   root mnemonic, and every call that derives seat keys and starts fedimintd.
2. For fresh onboarding, restore, startup, new-seat creation, restart, and
   supervisor respawn, identify what prevents overlap with the same guardian
   identity.
3. Attempt two concurrent `OnboardFromBackup` requests on the same root and on
   different roots, and restore while the original stays live. Confirm the
   code permits cross-root overlap with the flag set, then identify precisely
   why each trace violates A-operator.
4. Crash at each boundary from acknowledgement checking through identity and
   directory writes, response delivery, `Fleet::open_with_wallet`, and first
   child spawn. Look for an installed or live repeated identity whose trace
   did not pass the acknowledgement check.
5. Attempt to obtain a repeated identity through fresh onboarding, copied
   startup state, runtime seat creation, child respawn, and `RestartDKG`; report
   whether the attempt violates an axiom or reaches an unenumerated path.
