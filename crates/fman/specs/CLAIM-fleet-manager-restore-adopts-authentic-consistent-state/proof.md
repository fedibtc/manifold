# Proof: Restore adopts only authentic, consistent state



## Stale proof



> **Status: Unverified.** The stateless DKG lifecycle replaced
> `restored_with_config` and attempt-row consensus guards with one immutable
> `formed_seats` record reconstructed from an authenticated guardian archive.
> The authenticity argument below still covers the envelope and archive, but
> its Rust correspondence has not yet been renewed for that storage change.

Scope: `crates/fman/core/src/{backup,backup_queue,db,fleet,identity,onboarding,`
`restore,seat,supervisor}.rs`,
`crates/fman/core/src/db/seats.rs`, `crates/fman/core/migrations/**`,
`crates/fman/core/tests/{backup,onboarding,restore}.rs`,
`crates/fman/nostr/src/backup.rs`,
`crates/nostr-clients/src/nostr_relay_client.rs`, `Cargo.lock`

## Claim

A successful `OnboardFromBackup` never durably adopts either:

1. an identity, seat fact, payment record, guardian archive byte, or
   consensus-observed guard from a document that was not authentically produced
   under the recovered mnemonic's derived backup keys; or
2. an inconsistent set of authentic documents, including an older addressable
   event replayed in place of a newer publication when the replacement changed
   the state restored for that seat.

The adversary fully controls the configured Nostr relay: it may forge, replace,
reorder, withhold, duplicate, and replay events and may choose when to end a
query. The daemon may crash at any statement or await boundary during recovery
and installation. The host, operator-supplied mnemonic, database, and seat data
root are otherwise honest and exclusively operated by this implementation.

The enumerated adoption boundary ends at the identity/seat transaction and the
four restored guardian-config files. A subsequently started `fedimintd` may
populate its consensus database from federation peers; that separate network
input is explicitly outside the artifact domain above and is owned by axiom A3
(see the adjacent-boundary section below).

Pure omission is not quantified as forged adoption: a relay that withholds
documents and thereby makes recovery fail or produces an incomplete fleet is an
availability failure, recorded below. The second clause does include omission
used together with replay to make an adopted seat describe an older state.

## Axioms (trusted, not checked here)

- **A1 cryptography:** HKDF domain separation behaves as a PRF; SHA-256 is
  collision and second-preimage resistant; and NIP-44 v2 decryption
  authenticates its ciphertext. In particular, without the mnemonic-derived
  backup secret an adversary cannot make `unseal` accept a new or altered self-addressed ciphertext.
- **A2 durable single-host execution:** SQLite transactions and constraints,
  Tokio filesystem operations, and process-crash semantics behave as specified;
  committed writes survive a crash. No other process or operator mutates the
  database or seat directories during onboarding.
- **A3 fedimintd peer catch-up:** when a restored seat's pinned `fedimintd`
  populates its consensus database from hosted-federation peers, it validates
  that peer-supplied history under hostile bytes; the guarantee is bounded by
  the hosted federation's threshold honesty. No lemma in this record uses A3:
  it exists to own the adjacent boundary below, restating the composition
  root's A-fedimintd-protocol for this input (root owner ratification).

## Argument

**L1 (code) — the mnemonic fixes every authentication key.**
`RootMnemonic::derive_nostr_backup_keys` uses a fixed, backup-specific HKDF
info string. A `BackupIdentity` contains that key. Publication NIP-44-encrypts
each document to the backup identity itself and signs the resulting kind-37708
addressable event with that identity. Recovery derives the same identity from
the operator-supplied mnemonic; the relay cannot select another decryption key.
This uses A1.

**L2 (enum + code) — every relay byte path crosses the authentication gate.**
Regenerating relay-derived inputs and durable restore writers gives one ingress:
`RelayBackupArchive::fetch_documents` asks the shared relay client for events
filtered by the derived author and the backup kind, and returns those `Event`s
to `restore::recover_from_events`. The client enables subscription-filter
verification, but this is not the content-authentication gate: recovery consumes
only `event.content`, and each content must pass NIP-44 authenticated decryption
under L1, version checking, and typed JSON decoding. One failure aborts recovery
before any write. There is no other relay field or relay response copied into a
restored artifact. Thus a newly forged or altered document cannot reach the
recovered fleet (A1), even if its outer event is invalid.

The pinned relay-pool's signature admission is not reliable enough to claim
more. Its verification cache records an event id
before `event.verify()` succeeds; replaying the same invalid matching event can
then skip verification and reach the collector. That permits an unreadable-event
availability attack, not forged durable adoption: malformed content aborts and
copied valid ciphertext is still an authentic replay. An explicit
`event.verify()` at the FMan restore boundary would close this defense-in-depth
gap. Even a correctly verified author signature would not imply freshness: the
same signature that authenticates a publication also authenticates its replay.

**L3 (enum + code) — binding of each recovered artifact.** After L2, the only
relay-to-durable paths are:

- The identity row does not come from a document at all. `install` passes the
  parsed operator mnemonic to `install_restored_fleet`, which writes its
  canonical mnemonic phrase last in the transaction. Service and per-seat
  runtime identities subsequently derive from that mnemonic; the per-seat
  derivation is additionally scoped by the authenticated seat id.
- Each seat document supplies `seat_id`, `seat_no`, `quote_terms` (and hence
  `fi_id`), creation time, optional payment federation and signatures, optional
  guardian reference, and optional decommission time. `to_seat_facts` and
  `install_restored_fleet` copy precisely those fields into the seat, payment,
  and decommission rows. The database primary/unique/foreign-key constraints
  reject duplicate seat ids, seat numbers, and orphan payments.
- Archive chunks carry an embedded seat id, whole-archive digest, index, and
  count. Recovery groups them by that seat id. `GuardianArchiveChunk::join`
  sorts by index and uses the first chunk's count to require exactly indices
  `0..count`, while requiring one common digest, a whole-byte SHA-256 match, and
  valid archive JSON. It does not compare the `count` field on every later
  chunk; that omission cannot alter accepted bytes without defeating the final
  digest checks. Installation then requires every formed seat to have an
  archive under its own seat id, and `write_guardian_archive` re-hashes it
  against the digest in that seat's authenticated document before writing the
  four fixed filenames. Therefore
  chunks from different archives cannot be spliced, and an archive cannot be
  rebound to a seat document naming another digest (A1). Complete chunks not
  referenced by any returned formed seat are ignored and make no durable write.
- `restored_with_config` is true exactly when the authenticated seat document
  has a guardian reference. The database turns that boolean into
  `restored_with_config_at_ms`, which is the restored seat's
  consensus-observed / `RestartDKG` guard. No relay timestamp is adopted.

**L4 (code + schema) — installation cannot make a process-crash-created
mixture an onboarded fleet.** `recover` assembles and validates the
returned batch without writing. `install` first refuses an existing identity,
then checks every destination is absent and every formed seat has an archive. It
writes archive files only into newly created seat directories and only then
inserts all seat/payment/decommission rows and the mnemonic in one SQLite
transaction. A crash before the transaction commits leaves no identity and no
rows; any directory debris makes the retry refuse rather than overwrite it. A
crash after commit has the complete database transaction and only archives
whose writes returned before it began (A2). The named tests
`an_install_that_has_been_onboarded_refuses_to_be_restored_into` and
`a_restore_never_writes_into_an_existing_seat_directory` exercise the two
refusals; `a_restore_that_cannot_finish_leaves_the_host_un_onboarded` exercises
transaction rollback on a uniqueness error, not an actual crash or power-loss
boundary. Duplicate/contradictory seat documents may cause repeated archive
writes and leave directory debris before uniqueness rejects the transaction,
but cannot produce a successfully onboarded database. No claim is made here
about un-fsynced archive files surviving power loss.

L1–L4 establish resistance to forged content, altered ciphertext, archive
chunk splicing, cross-seat archive substitution, and crash-created adoption.
They do not establish the second clause of the claim.

**L5 (enum + code) — there is no freshness or fleet-consistency gate.** The
restore query filters only by author and kind. Recovery does not validate an
event's addressable coordinate, compare Nostr `created_at` values, retain one
maximal event per coordinate, require a publication sequence or hash chain, or
compare the returned head with a mnemonic-bound checkpoint outside the relay.
Documents carry no monotone revision or fleet-wide snapshot id. The query's
EOSE and resource checks establish only that the relay finished the answer it
chose to send; they cannot prove that it sent its newest stored event. Digest
checks bind archive bytes to whichever seat document was returned, not that
document to the latest publication.

**Counterexample (falsifies clause 2).** This is exploitable by the relay
alone once both authentic revisions have been published: it requires no
malicious operator, federation peer, host process, or cryptographic break.

1. A seat reaches consensus. The FMan authentically publishes archive chunks
   and seat document `D1`, which names their digest and has no decommission
   timestamp.
2. The operator decommissions the seat. The FMan authentically publishes the
   replacement `D2` at the same addressable coordinate, now carrying
   `decommissioned_at_ms`.
3. On restore, the controlled relay withholds `D2`, replays signed `D1` and its
   authentic chunks, and sends EOSE. All gates in L1–L4 pass.
4. Installation commits the recovered mnemonic, seat and archive with
   `restored_with_config_at_ms` set but no decommission row. The restored fleet
   therefore starts a guardian the authentic newer document said was retired.

The operator cannot distinguish this answer from a restore performed before
step 2: neither the mnemonic nor any non-relay state commits to `D2`, and the
restore response exposes only seat and formed counts. Equally, for two seats
the relay can return the newest document for one and an older authentic
document for the other; no fleet-wide epoch rejects that temporal mixture.
Returning both replacements for one seat normally fails on database uniqueness,
but the adversary need only return the selected older one.

A second counterexample replays the authentic pre-consensus seat document after
that seat has formed, while suppressing the formed replacement and archive
chunks. Restore succeeds as paid-but-unformed, installs neither archive nor
`RestartDKG` guard, and permits another ceremony even though the original seat
already owns consensus key shares. This is again replay-changing-adopted-state,
not pure omission.

## Residual windows (accepted, outside the claim)



## Adjacent composition boundary (owned by A3)

**Peer-assisted consensus catch-up.** After restore installs the enumerated
artifacts, the child may durably populate its separate consensus database from
hosted-federation peers. The composition root now treats those peers as
untrusted. No peer byte enters `recover` or `install`, and the consensus
database is deliberately absent from the backup, so this record neither trusts
peers nor proves fedimintd's threshold/history validation. The root must not
import this record as covering peer-derived consensus state; that input is
owned by axiom A3 above — a trusted premise about the pinned fedimintd,
bounded by threshold honesty, not a proof. This is outside the exact
restored-artifact claim, not evidence that hostile peers are safe beyond
what A3 assumes.

## Weakest links



## Open recovery premises

EOSE does not establish a complete relay snapshot. Restore therefore remains unverified unless the exact-shape mechanism receives a complete honest, quiescent answer and detects whole-seat omission. SQLite/filesystem durability, retry-safe staged adoption after process crash or ENOSPC, and same-second addressable-event ordering remain load-bearing premises.
