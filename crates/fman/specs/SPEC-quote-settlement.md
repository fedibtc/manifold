# SPEC-quote-settlement: Quote IDs determine settlement outcomes

## Record justification

The contract spans quote issuance, payment verification, SQLite admission, and FI RPC replay behavior, so no single implementation artifact can own it coherently.

`CreateSeat` answers every question from the quote id and one durable random
epoch. There is no clock on the admission path and no expiry. Nothing about a
refusal is stored, because a refusal can be recomputed; the only durable
settlement outcome is a seat row.

## Quote terms

The signed terms contain the **offer epoch** current at issue and the FI's
**refund issuance**, with an FI-chosen nonce binding its derivation. A quote
carries no time at all. `CreateSeatRequestKind` contains only the payment.

The FI can commit to its refund outputs at quote time because it already has
everything they depend on: the price is in the `Plan` it is sending, and the split
from price to denominations is deterministic. A stale price triggers the
offer-mismatch rejection.

Since `QuoteTerms` embeds the request, the offer epoch and refund issuance
are hashed into the quote id. That is what makes the refund transaction
computable from the quote alone.

## The offer epoch

One random 32-byte value standing for both the FMan's capacity and the terms it
sells on. A quote whose epoch differs from the current epoch is refused,
permanently. Two things replace it with fresh randomness, each in the
transaction of the write that motivates it: the acceptance that takes the last
free slot, and a change to `QuoteSettings`.

`QuoteSettings` holds the operator-controlled inputs to quoting and nothing else.
Quote composition takes one and has no other route to operator state, so this
type bounds the settings that invalidate a quote. Replacement is on an actual
change, structurally compared — an idempotent write does not invalidate every
outstanding quote.

An offer epoch is more precise than a timer: a timer refuses quotes that
are still exactly on offer for being old, and honours quotes whose price the
operator changed a second after issuing them. The FMan honours an arbitrarily
old quote while nothing has changed, which is the correct protocol outcome —
those are still its terms. The FI refreshes unpaid paid quotes before
presentation. It replays a paid quote exactly once its wallet reports that
payment started, because replacement would strand the quote-bound funds. It
also replays a stored free quote exactly until the FMan signs a refusal,
because a lost acceptance response may otherwise make a replacement quote
allocate a second quote-derived seat.

## What `create_seat` writes

Only acceptances write, and each writes once. A replay of a settled quote
re-signs its acceptance; a stale-epoch quote is refused, writing nothing and
starting nothing. One read snapshot can resolve either outcome without waiting
for the writer. A request that is absent and current then enters one immediate
SQLite write transaction, rechecks replay and epoch, checks capacity and the
lifetime port cursor, inserts the seat, and replaces the epoch with fresh
randomness if that insert consumed the last slot.

SQLite's writer reservation guards the admission decision, port cursor, and
insert against every settings or admission writer. Acceptance signing occurs
before that transaction because the signed commitment must exist before the
atomic insert; no wallet call occurs inside it. After commit, an exclusive
registry entry publishes or repairs the one live runtime for the durable seat.

## Settlement

`create_seat` atomically records typed claim evidence with the seat.
The wallet-owned reconciler scans nonterminal rows, derives the deterministic
Fedimint operation id, consults the operation log before handoff, and awaits the
operation's terminal success or rejection. Startup and periodic scans, plus a
wake hint on acceptance or replay, retry recoverable failures with bounded
concurrency and timeout. Each payment therefore converges on one Fedimint
operation without storing a duplicate in-progress journal in core.

It chases claims only. The FI is the party motivated to collect a refund, and it
can replay `CreateSeat` for its signed refund at any time.

## Schema

Seat rows are the sole durable admission outcome. Their primary-key `quote_id`
stores the quote's 32 bytes. `SeatId` is a distinct accepted-seat type wrapping
that `QuoteId`; its wire, display, and key-derivation form is always the
64-character lowercase hex encoding, so the relationship cannot drift inside
FMan and `/` cannot enter a seat's key-derivation label. The FI treats the
FMan-issued seat id as opaque. Paid seats have a foreign-keyed claim row holding
mint-v1 or mint-v2 evidence and a terminal claim outcome and timestamp.
The payment federation's public invite (which carries its federation id),
issuance material, and module identity live inside that typed evidence;
commercial lifecycle facts stay on the seat.
SQLite stores the typed evidence as one CBOR blob in `ecash_claims`, using the
same serialization as the CBOR backup document. There is no second persistence
shape to translate or evolve.
Free seats have no claim row. The database also stores one random
offer epoch. A refusal has no row, so there is no accepted-and-refused overlap
to scan and no refusal record to expire.

The FI can therefore predict the seat id before paying. This reveals only the
public derivation label: deriving seat keys and `api_auth` still requires the
FMan's root mnemonic.

## Invariants

1. **`GetQuote` reads capacity, the epoch, and `QuoteSettings` in one read
   transaction, and issues nothing at zero capacity.** Split them and the FMan signs a quote
   priced at terms its stamped epoch does not name, or a current quote for a
   full FMan.
2. **The seat insert and epoch replacement are one transaction.** A crash
   between them leaves zero slots with a still-current epoch.
3. **An epoch replacement is durable before any refusal that depends on it.** Both
   replacements satisfy this structurally, by committing in an earlier request's
   transaction than any refusal they enable.
4. **A current-epoch quote is guaranteed a slot**, so the capacity check
   inside the lock is a fail-closed assertion, not a branch.

## Consequences for the claim records

Two parts of
[CLAIM-fleet-manager-quote-settlement-exclusive](./CLAIM-fleet-manager-quote-settlement-exclusive.md)
hold structurally: the absence of a second kind of outcome row gives outcome
exclusivity, and refund canonicality is definitional. Invariant 3 supplies the
remaining refusal-ordering obligation. Payment claiming starts only from a
committed seat's claim evidence; that separate ordering is not derived from
the admission invariants above. No refusal reason depends on time, so nothing in
the settlement argument rests on clock behaviour.
`CLAIM-fleet-manager-preserves-published-guardian-data` is untouched.

## Alternatives considered

**A refund ledger.** In this alternative, `CreateSeat` holds the admission write transaction
across a ledger read, the decision, signing, and the insert; a refusal writes a
row storing the refund bytes; settlement is a detached task whose outcome is only
logged. Such a ledger is necessary when capacity refusals can flip — a
decommission can free a slot, so a quote refused on one request could be accepted
on a later request, and the FMan would claim notes it had already refunded.
Monotone admission avoids both the job and the record.

**Leaving `refund_issuance` in `CreateSeatRequestKind`.** Costs no wire change,
and multiple signed refusals would be harmless, since they spend the same notes.
Rejected because the FMan retains nothing for a refusal, so the only holder of the
FI-signed request is the FI — which can sign a different one after the fact and
point at a mint history that disagrees with it, leaving the FMan no answer.
Hashing the outputs into the quote id lets an arbiter clear an honest FMan from
the quote alone.

**A countersigned draft quote.** Considered when it appeared the FI could not
compute refund denominations before learning the price. It can. No draft,
signature, TTL, or extra round trip is needed.

**Keeping quote expiry.** On the FMan side it was one branch bounding the price
commitment, and the epoch does that better. It was also not free: refusals
must be recomputable, and `SystemTime::now()` can move backwards and un-expire a
quote already refused, so honouring expiry safely required a durable monotone
clock floor, a tick to advance it, and a commit-before-publish rule on that tick.
Two traps if a clock is ever reintroduced — gating on `max(now(), floor)` rather
than the floor alone leaves a window the tick interval has to shrink to zero to
close, and raising the floor only on refusals never advances it while idle.

**Bumping on any operator settings write.** Simpler to state, but sweeps in knobs
that have nothing to do with quoting, and only coincidentally matches the current
fields. `QuoteSettings` names the boundary instead.

**Garbage-collecting refusal records.** Unnecessary once refusals are monotone:
there is no record to collect.

**Chasing refunds in background tasks.** Would mean retaining payment evidence
for refused quotes so a task could rebuild a transaction the FI can ask for on
demand. An FI that walks away leaves its notes unclaimable by the FMan either way.

**Retaining refusals as dispute material.** Every artifact a refusal dispute turns
on is signed and every derived value deterministic. What is given up is unilateral
enumeration — "list every refusal last quarter" with no counterparty present. That
is accounting, and belongs in an append-only log beside the decision where it may
be lossy, never in a table the settlement path reads.

Governs the admission and settlement behaviour in
[SPEC-locked-payment](./SPEC-locked-payment.md) and
[SPEC-fi-rpc](./SPEC-fi-rpc.md) *CreateSeat*; refines the outcome-write
discussion in [ARCH-fleet-manager-storage](./ARCH-fleet-manager-storage.md).
