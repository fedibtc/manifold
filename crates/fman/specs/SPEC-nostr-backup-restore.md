# SPEC-nostr-backup-restore: Mnemonic-only fleet recovery through Nostr

## Record justification

Mnemonic-only recovery necessarily spans fman-core's data path, the FMan Nostr
adapter and event kind, the admin verb, and the external `fedimintd`
configuration-rebuild property, so no local artifact can own the contract.

The FMan publishes its irreplaceable state as encrypted Nostr documents that
only its own root mnemonic can find and read. An operator who has lost the host
entirely restores the fleet from the phrase alone: no exported archive, no file
the operator had to remember to copy.

## What recovery has to reproduce

A seat has two eras, and they are at risk of completely different things.

**Before consensus**, the only irreplaceable thing a seat owns is the FI's
money. No key shares exist yet, so nothing on the `fedimintd` side is worth
protecting: a restored unformed seat that starts a fresh ceremony
simply runs the ceremony again, which is a correct outcome and not a loss. This
is why the payment is backed up from the moment the seat is created, and why no
ceremony state is backed up at all.

**After an FI response carries the federation invite**, the seat acquires the
loss
[CLAIM-fleet-manager-preserves-published-guardian-data](CLAIM-fleet-manager-preserves-published-guardian-data.md)
is about.

Four kinds of state, and two of them are genuinely at risk:

- **Derived** — every key in
  [ARCH-fleet-manager-identity](./ARCH-fleet-manager-identity.md). Recomputed
  from the mnemonic; never backed up.
- **Guardian config** — each seat's DKG key shares and the federation's
  consensus config, written once by `fedimintd` when the ceremony completes.
  Not derived, not held by peers, not reconstructible. **This is one of the two
  things the backup exists for.** The four files are carried together as one
  immutable **guardian archive** — `private.encrypt`, `private.salt`,
  `local.json`, `consensus.json`, exactly the set `fedimintd`'s own
  `download_guardian_backup` exports — because they are written at one moment,
  restored as a set, and useless individually.
- **Accepted payment material** — typed mint-v1 or mint-v2 claim evidence for
  each paid seat, including the public invite needed to rejoin its payment
  federation independently of the current setup-payment set. It cannot be
  fetched again: obtaining it means submitting
  an issuance backed by inputs the FI has already spent, and the escrow-phase
  notes they cover sit outside the fedimint client's own recovery scan. Losing
  them loses the FI's money. **This is the other.** It exists from the moment a
  seat is created — long before DKG — so it is at risk for the whole interval an
  FI takes to run the ceremony, which may be days, or forever.
- **Fleet facts** — a seat's creation facts. Small, and meaningless without the
  seat material they accompany, so they ride along in the seat's own document
  rather than anywhere else. The signed creation commitment is not among them:
  it is a signature over "this quote was accepted", re-signed from the seat row
  by the same root-derived key ([ARCH-fleet-manager-storage](./ARCH-fleet-manager-storage.md)).
- **Consensus database** — recovered from peers, so deliberately not backed up.
  A restored guardian replays threshold-signed session history from the
  federation. This is what keeps the backup small enough to publish at all.
- **Federation consensus config** — public, identical for every guardian, and
  **not obtainable**. It is most of the archive's bytes, which is why the
  archive spans multiple events at all.

  The intent was to refetch it from a surviving peer. There is no endpoint that
  allows this. The one API that serves a guardian's `consensus.json`
  (`download_guardian_backup`) authenticates the request against the *target*
  guardian's `api_auth`, which a restoring FMan does not have and cannot derive
  — it holds only its own. The single unauthenticated server-config endpoint
  returns a legacy hash, not bytes. Nor can the file be rebuilt from the client
  config an invite code yields. That projection is a re-encoding, and what it
  omits is what a *server* needs: `code_version`, `broadcast_rounds_per_session`,
  the TLS certs, each peer's Iroh p2p key and websocket URL, and each module's
  consensus config in its erased server encoding. How much of a module survives
  is module-specific — the mint's client config carries the whole of
  `MintConfigConsensus` (the per-peer key shares that are most of the file's
  bytes), while the wallet's drops `peer_peg_in_keys` and `default_fee`. Even
  where a value survives, reassembling the file would mean the FMan parsing and
  re-encoding fedimint's config types, which it does not link
  (ARCH-fleet-manager, `fedimint_api`).

  The remaining options were to carry it or to make the operator keep a copy of
  a file. The second contradicts the whole record — "no exported archive, no
  file the operator had to remember to copy" — so it is carried, inside the
  guardian archive. At roughly a hundred kilobytes the archive does not fit one
  padded event, so it is sealed whole and its ciphertext cut into fixed-size
  slices, each its own event. Nothing parses any of it.

  `download_guardian_backup` is not used to obtain the archive even for this
  FMan's *own* seats: it needs a live child API, and it re-encrypts the private
  config under a fresh salt on every call, so no two answers are the same bytes
  — which would turn an immutable archive into a payload that changes every
  time it is published. The files are read from the seat's data directory
  instead.

  The slices are addressed beside the seat they belong to and carry no
  structure of their own — no index, no count, no digest. The coordinate
  orders them, the AEAD tag refuses a reassembly that is incomplete,
  reordered, or spliced from different seals, and the digest the seat's
  document records binds the opened archive to *that* seat; the write that
  installs the files checks that digest once more. Nobody passes a digest in
  beside the bytes — an untrusted copy is safe to accept only because of
  those checks, so no restore path is in a position to skip them.

## Documents

Backups are addressable Nostr events authored by a **dedicated backup identity**
derived from the mnemonic, deliberately distinct from the FMan's service
identity so that discovery and trust surfaces stay unlinked from recovery
material. Payloads are CBOR, framed with their length, and sealed with
XChaCha20-Poly1305 under its own mnemonic-derived symmetric key — the same
shape as fedimint's client backup — with the event family bound in as
associated data. The frame has one canonical spelling — exactly one CBOR item
in the declared length, zero padding after it, no more padding than reaching
whole events requires — and readers refuse any other, so a payload has a
single sealed form. The events are readable only by the holder of the phrase.

Everything in this section that exists only because the storage is Nostr — the
sealed payloads, the blinded coordinates, the archive slicing, the padding,
and the schema version — is owned by the Nostr adapter
(`fman-nostr`), behind the same publish/recover boundary that keeps relay
connections out of seat logic. fman-core owns *what* is irreplaceable and
assembles it; the adapter alone decides how it is laid out on a relay and how
it is read back.

- **One event per seat**, carrying that seat's durable facts, its accepted
  payment material, and — once the ceremony has produced one — a *reference* to
  its guardian archive: the archive's digest and the formed federation's invite
  code. No config bytes are in it. A retired seat's event carries its
  decommission time, so a restore does not resurrect a guardian into a
  federation the operator deliberately left.

  The archive is not in the document because the two have opposite lifetimes.
  A seat's document is mutable — republished when lifecycle facts change, when the
  invite is observed, when the seat is decommissioned — while `fedimintd`
  writes its config files once, at the end of the ceremony, and never again.
  Welding the immutable payload into the mutable document would re-publish a
  hundred kilobytes of key shares on every state change, for no new
  information. So the seat's document names the archive and the archive is its
  own family, published once.

  **No ceremony state is carried.** Guardian names, setup codes, attempt
  history, and the consensus-observation timestamp are all absent. A restore
  needs exactly one bit of the ceremony — whether this seat has formed — and the archive reference's
  *presence* is that bit: `fedimintd` writes its config when the ceremony completes
  and at no other time. Recording an observation timestamp beside it would be a
  second source of truth for one fact, and before consensus there is no fact to
  record.

  **The event's addressable coordinate is blinded.** It is
  `HMAC(tag_key, "seat:" || seat_id)` under a third mnemonic-derived key, not
  the seat id. A seat id is the canonical hex of a quote id, which the FI that
  bought the seat holds: an unblinded coordinate would let that FI query the
  relays for its own seat and so resolve the FMan's backup identity, which is
  precisely the link the separate derivation exists to break. The document
  family is inside the MAC rather than a plaintext prefix, so the archive
  family neither collides with this one nor announces itself on the relay.

  A seat's event is published more than once over its life because those parts
  become available at different times; each publication replaces the last at the
  same coordinate, so every one carries the whole document rather than a delta.
  Each publication is therefore rebuilt from durable state rather than from
  whatever the publishing call site happens to hold — a partial document would
  silently erase what an earlier publication had already made durable. That is
  structural, not a rule each publishing site follows: one assembler
  (fman-core) reads the database and the seat's data directory and returns the
  seat's whole publication — its document plus, until confirmed, the archive
  bytes it names — and the storage adapter (fman-nostr) alone turns that into
  events, so the publisher chooses neither the contents nor the encoding, and
  cannot express a partial document.

  The invite code is the seat's link to the federation it guards. It is written
  set-once with the immutable formed-seat row when the driven child reports its
  final configuration persisted ([ARCH-fleet-manager](./ARCH-fleet-manager.md)).
  The same row serves document assembly, `GetInviteCode`, and status
  ([SPEC-seat-lifecycle](./SPEC-seat-lifecycle.md)). Restore reconstructs that
  row from an authenticated guardian archive reference.

  The document carries the seat id and FI id directly. The seat id is the
  canonical hex of the quote id. The FI id is an independent seat fact because
  the durable seat keeps only the lifecycle projections of the accepted quote,
  not its full signed terms. Restore uses both values to reconstruct the seat.

**There is no fleet-wide event.** Operator settings and offered plans are the
operator's to re-enter, and the offer generation must *not* survive a restore:
a fresh install's generation refuses every quote issued before the loss, which
is the wanted outcome — those quotes were priced against an advertisement the
operator has to re-make ([SPEC-seat-lifecycle](./SPEC-seat-lifecycle.md)).
Backing it up would restore the staleness it exists to prevent.

**Guardian archive slices** are the second family: the archive is sealed
whole, as one AEAD blob, and its ciphertext cut into fixed-size slices, each
an event addressed beside that seat's own document
(`HMAC(tag_key, "archive:" || seat_id || ":" || index)`). A slice carries
nothing but its cut of the ciphertext: the coordinate is the only record of
order and membership, and the tag on the reassembled whole is the only
integrity check it needs. The slices are published before the document, so a
seat document on a relay is never the newer half of a pair — it names a
digest, and a digest verifies an archive rather than conjuring one. The
archive never changes, so if a publication does repeat, it rewrites the same
archive at the same coordinates (the sealed bytes differ — sealing is
nonce-randomized — but what they carry does not).

One event per seat rather than one event per fleet is a scaling decision: a
single document would cap durable seat capacity on a relay's event-size limit, and the
backup format must not constrain how many seats an operator may host. Every
event is one size — documents by padding before sealing, slices by choosing
the archive's padding so its ciphertext cuts evenly — and nothing on an event
marks its family: the reader tells them apart by the cipher. The two families
are sealed under distinct associated data, so a document opens only in the
document domain and an archive — even one small enough to seal into a single
slice — only in the archive domain; a slice of a larger seal does not verify
on its own at all (a false authentication is a ~2^-128 event, not an
impossibility). An observer can distinguish neither family, and can count
neither.

One schema version covers the whole format, carried inside the seat document's
sealed envelope and nowhere else: the version says what a reader must
understand before it can read *anything*, every archive is reached through
some seat's document, and a second copy would be a number with nothing to make
it agree with the first. A version this build does not know is refused before
the body is parsed, rather than parsed into whichever fields still happen to
match.

Restore enumerates seats by querying the backup identity and kind. There is no
index document to keep consistent with the events it would describe.

The mnemonic itself is never published. A backup is inert without the phrase,
and the phrase alone — with no backup — cannot reconstruct a guardian.

## Publication is reconciliation, and nothing waits for it

The relay is **semi-trusted**: it sees only ciphertext, event count, and
timing, and it is trusted to keep serving the latest event published at each
coordinate. That trust is what makes a *confirmed* publication — one whose
event was read back from the relay after writing — a durable fact worth
recording. Each seat's last confirmed publication is recorded as the SHA-256
of the plaintext document (`seat_backup_publications`; the sealed bytes cannot
serve, because sealing is nonce-randomized, so they are not a stable
identity), plus the digest
of the guardian archive once its slices have been confirmed. The record is
scoped to the schema version that wrote it: the version lives outside the
hashed plaintext, so after a version bump an unchanged document would
otherwise look confirmed while the relay serves events the new build's own
restore refuses. A record under another version is no confirmation, and the
whole publication — archive slices included, since a reader reassembles them
by the rules the version names — republishes under the current one.

Dirtiness is therefore **derived, never tracked**: a seat needs publishing
exactly when the document assembled from its durable state no longer hashes
to what the relay was last confirmed to serve. One background worker scans
every seat on a slow cadence and converges the difference; a state transition
— seat creation, the first observation of consensus, a decommission — marks
the worker purely so the next scan happens promptly. Correctness belongs to
the scan, which needs no mark, so there is no queue to lose, no startup
republication of unchanged seats, and no way for a missed mark to mean a
missed backup. A scan reads only the database and seat data directories,
never a child's API — and once a seat's archive is confirmed, its recorded
digest stands in for the files (they are written once and never change), so
steady-state scanning costs a few local reads per seat.

The ordering within one seat's reconciliation is publish, read back, then
record. A crash between the confirmation and the record costs one redundant
republication of an identical document at the same coordinate — the cheap
side of the ambiguity — and never a record claiming the relay holds something
it was not seen to hold. Archive slices not yet confirmed are republished
even under an unchanged seat document, for the same reason.

**Nothing waits on a relay** — no request path, no spawned task, no state
transition. Creating, probing, and retiring a seat return at local speed. The
daemon's own start is included: the relay connection is made by the first
publication, so an unreachable relay never stops an FMan running the
guardians it already has. Every relay call the worker makes carries its own
deadline, and failed scans retry with jittered exponential backoff — the
relay is shared by the fleet, and synchronized retry pulses would hit it
exactly when it is weakest. A restore is the one exception to nothing
waiting, and it is not a wait but the operation itself — it reads the
documents from the relay before anything exists.

The operator can see all of this rather than infer it: each seat's summary
carries its last confirmed publication (and whether the archive is among what
was confirmed), and the seat listing carries the worker's last completed
scan, whose staleness is itself the signal that the worker is wedged.

An earlier design made publication a *gate*: the payment hand-off waited for
the seat's document to be confirmed, and a formed seat's invite code was
withheld until its guardian config had been published. Both are gone, and the
reasoning is worth keeping because it is the reasoning for any similar
proposal.

- **A gate cannot close the window it is aimed at.** `fedimintd` writes key
  shares when the ceremony completes, and the backup is after-the-fact by
  construction. There is always an interval in which the only copy of a
  guardian's shares is on one disk. Withholding an invite does not shorten it;
  it only hides the seat while it elapses.
- **The invite code is not the FMan's to withhold.** It is public, and every
  other guardian in the federation serves it. Withholding this guardian's copy
  denies the FI nothing it cannot get elsewhere, while making a healthy
  guardian look broken.
- **A confirmation is a fact about the past, not a promise.** A read-back
  says what the relay served at that moment; whether it still serves it later
  is the semi-trust assumption, a property of the deployment's relay. Gating
  an irreversible act on a third party's future behavior buys less than it
  appears to.
- **The trade runs the wrong way.** The gate exchanged a rare failure (host
  destroyed in the seconds between a claim and its publication, where the
  database is the copy that was going to be relied on anyway) for a common one
  (relay unreachable → FI's money unclaimed, invite withheld, seat apparently
  broken). It made an optional second copy a prerequisite for the FMan's core
  function.

What survives from that design is one refusal. A seat that has ever run
consensus must not publish a document without its guardian config; a
publication that would is refused before it reaches the relay, and the seat
stays pending. That is not a gate on anything else — it is what keeps the
seat on the worker's pending list while `fedimintd` is still writing its
config out, instead of leaving a seat that looks published while the only
copy of its shares is on one disk. The requirement derives from the durable
formed-seat row — the set-once record the driven seat loop writes
([ARCH-fleet-manager](./ARCH-fleet-manager.md)), or the same fact a restore records — never from the archive files
themselves, which are exactly the state whose absence the refusal exists to
notice. A daemon restart cannot forget it.

The formed row gates the archive from the other side: config files on
disk *become* the seat's guardian archive only once the driven child reports
that its atomic config persistence completed and FMan durably mirrors that
fact, so a document assembled before it carries no archive reference. The final data directory makes a completion race self-heal rather than allowing
cancellation to erase it throughout the event-delivery crash window ([SPEC-seat-lifecycle](./SPEC-seat-lifecycle.md));
the durable formed row associates the immutable directory with this seat's
completed ceremony before backup publishes it. A document's archive reference
is therefore present exactly when the formed row is durable. And because the
plan's observation check and the
publish are not one atomic step, the worker re-derives the requirement after
the publish and before the record: formation landing while a
share-less document is in flight leaves the seat pending — recording that
publication would suppress the very republish that adds the archive.

Publications converge rather than being atomic with the writes they
describe: a document assembled a moment before the federation invite was
recorded carries the config without it, and the next scan — cut short by the
mark that recorded the invite — republishes.

The guarantee this leaves is that the relay converges on each seat's current
document, promptly after a change and verifiably at each confirmation. A
seat can still be paid for, formed, and lost with nothing published; the
answer to that is a reachable relay, not a wedged FMan.

## Restore is onboarding

**A host is set up once**, as either a new install or a recovery of an old one.
After that it is an ordinary running FMan and restore is not available to it
again. This is the whole shape of the feature, and everything else about restore
follows from it: it runs against a database with nothing in it, so it inserts
rather than reconciles, and creates seat directories rather than writing into
any.

**Onboarding is a phase of the daemon's start**, and restore is one of its two
answers ([SPEC-admin-socket](./SPEC-admin-socket.md)). A daemon whose data root
holds no identity has nothing to run — every key derives from a phrase it does
not have — so it binds the admin socket and waits for the operator to say which
Fleet Manager this host is:

```
  fman-cli --data-dir <path> onboard new
  fman-cli --data-dir <path> onboard restore --mnemonic-file <file> \
      --acknowledge-original-host-is-gone
```

Nothing mints a mnemonic implicitly. The identity row records the operator's
choice, so a restore that finds one refuses, and its primary key makes that
choice happen once. Restore then advances durable onboarding to Holder
authorization. The operator must reacquire authorization and configure the
initial price and capacity; restored guardian children remain stopped until
those stages complete. The offer itself is operator policy and is not restored
from guardian backup documents.

Restore takes the mnemonic, enumerates and decrypts the documents, reassembles
each seat's guardian archive from its slices, writes each seat's config files
into a new seat directory, and rebuilds the fleet facts. Beside the archive's
four files it writes one the backup deliberately does not carry:
`password.private`, the seat's `api_auth`, re-derived from the mnemonic — the
exact bytes `fedimintd` itself writes there after a ceremony, and without
which `fedimintd` treats an existing config as absent and enters a fresh
ceremony instead of serving the restored one. It also seeds each
seat's publication record with the hash of the document it just fetched — the
relay demonstrably serves exactly that document — so the restored fleet's
first scan republishes nothing. After the remaining onboarding stages complete,
the daemon starts that fleet and each guardian catches up from its peers.

**A partial answer is an error, never a smaller fleet.** Because it happens
once, restore has no second attempt in which to notice something was missed, so
every way of reading less than the whole backup fails instead: the enumeration
is accepted only if the relay signalled it was complete — a timeout, a dropped
connection, or the candidate cap refuses the restore — and a document that will
not decrypt or parse refuses it too. Relay answers are signature-verified and
filtered by author, and only the phrase holder can author under the backup
identity, so an unreadable document is this fleet's own rather than a stranger's
noise, and no third party can push a restore into either failure.

The whole fleet is assembled and checked before anything is written: the install
must have no identity, every seat directory must be absent, and every formed
seat's guardian archive must be present and must match the digest that seat's
document names. A restore does not stop halfway having
created some guardians and not others.

**The identity row and transition to the Holder-authorization stage are written
in the same transaction as the seats.** An interrupted restore must be retryable
without filesystem surgery, so a host interrupted mid-install remains at the
identity-choice stage and can restore again, rather than appearing to have a
partial recovered fleet.
Across the filesystem writes that precede the transaction, retryability comes
from staging and adoption: seat directories are written under a reserved
staging directory that the next attempt wipes, and renamed into place so a
final seat directory only ever appears complete; a final directory that
already exists is adopted if and only if it is exactly what a staged write
renames into place — the archive files hashing to the digest the seat's
document names, the re-derived password, and nothing else. That is exactly
what an interruption between the renames and the transaction leaves, and
nothing a foreign directory — including a live guardian's, whose consensus
database is precisely the extra content the shape check refuses — can satisfy.

Three constraints, and two of them are enforced rather than documented:

- **Restore never writes into an existing seat directory.** It creates seats,
  adopts by a read-only shape-and-digest check, or refuses. Restore therefore
  adds no guardian-directory deletion site relevant to
  [CLAIM-fleet-manager-preserves-published-guardian-data](CLAIM-fleet-manager-preserves-published-guardian-data.md),
  and a misdirected restore cannot destroy a live guardian.
- **Restore refuses an install that already has an identity.** An identity row
  exists only because an operator onboarded this host, and burying that is
  never what they meant.
- **Restore requires an explicit acknowledgement that the original guardian is
  permanently offline.** Two hosts running one guardian identity equivocate, and
  mnemonic-only recovery makes standing up a second copy easy. No state the
  daemon can observe answers this, so the operator asserts it; it is the one
  constraint that cannot be moved off a human.

A restored guardian archive installs the same set-once formed record as driven
DKG, so a later ceremony cannot replace the recovered federation
([SPEC-seat-lifecycle](./SPEC-seat-lifecycle.md)). The backup deliberately carries
no ceremony state; the formed invite is the only lifecycle fact restoration
needs to reconstruct.

A restored seat *without* a guardian config takes the opposite treatment: it is
restored as a paid-but-unformed seat, and a later DKG ceremony is accepted. That
is deliberate. The seat's payment and commitment are intact, which is the whole
of what was at risk before consensus, and re-running the ceremony is how such a
seat is meant to reach a federation.

## Boundaries

- **Per-guardian insurance, not federation insurance.** If a threshold of
  guardians lose their databases at once, no config reconstructs the consensus
  history, and client ecash backups held in it are gone with it.
- **Catch-up is not instant.** A restored guardian replays session history from
  peers; for a long-lived federation that is the dominant cost of recovery.
- **One equivocation window remains.** A guardian that had already contributed
  to the in-flight consensus session and returns without that state may conflict
  with itself for that session. Finalized sessions are threshold-signed and
  replayed canonically, so the exposure does not extend behind them.
- **Relays are semi-trusted, not audited.** A publication is confirmed by
  read-back at the moment it is made, and the recorded confirmation is what
  keeps unchanged seats from republishing — but nothing re-verifies that the
  relay still serves an event months later. That continued service is the
  semi-trust assumption itself. The relay is the one the Manifold environment
  profile names — the same one this FMan advertises on — so recovery odds are
  a property of that deployment's relay, not of a per-host flag. This is the
  property the removed gates pretended to supply.

## Accepted disclosures

The backup identity is unlinked from the service identity by derivation, and
each event's coordinate is blinded, so nothing observable names an FMan, a seat,
or a federation. What remains observable to someone who has *already* resolved
an FMan's backup pubkey is the **number** of seat documents under it and the
**timing** of publication.

Neither is confidential by this record alone: the advertisement does not
disclose capacity ([SPEC-advertisement](./SPEC-advertisement.md)). Resolving the
backup pubkey requires the
mnemonic: an observer must already be enumerating a kind with no author to
correlate it to. Closing the timing channel outright would mean deliberately
delaying publication, which trades the one property publication has — that it
happens promptly — against an observer who has already lost the phrase, so it
is accepted rather than traded for.

Event kinds and tags are allocated in
[SPEC-fman-nostr-events](../../nostr/specs/SPEC-fman-nostr-events.md). The
shipping boundary this record moves is
[ARCH-fleet-manager-product-boundary](./ARCH-fleet-manager-product-boundary.md).
