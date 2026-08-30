# SPEC-locked-payment: Key-locked ecash seat payment

## Record justification

The contract spans FI payment construction, FMan quote signing and offline
verification, two Fedimint mint generations, durable settlement, and wallet
recovery, so no single implementation artifact can own it coherently.

A seat is bought by issuing Fedimint ecash notes **locked to keys the
FMan provided**. A note's nonce is a public key and spending it requires
a fedimint transaction signed by that key, so notes issued to FMan-derived nonces
can never be spent by the FI or anyone else. The FMan verifies a payment
entirely offline; a seat exists only after payment is complete; there
are no FMan seat or capacity reservations. The FI payer wallet does create a
local, durable aggregate reservation before it authorizes outputs. That record
holds fee-aware debit allocations and the caller's required balance floor, binds
every semantic quote id to its exact foreign-output plan, and gates ordinary
wallet spending. It is strictly payer-side crash/recovery state and gives the
FI no claim on an FMan slot. Before starting each member, the payer re-plans
against the wallet's current note tiers and proves the resulting debit leaves
every other held allocation and the balance floor funded; change returned by
earlier members can therefore neither cause a false failure nor consume a
sibling hold. Both mint generations are supported — the scheme
relies only on note structure and on the mint signing whatever blinded
nonce a transaction pays for — and no fedimintd server changes are
required.

## Quote

`GetQuote` is a pure function; the FMan stores nothing. The response is
a signed quote: public terms (plan, price, offer epoch, a random quote
nonce, and — for paid quotes — the payment terms: the payment
federation and the issuance set of blinded nonces covering the price in
fixed denominations, each with its denomination and, for mintv2, its
public random tweak). The quote is its terms alone — **nothing travels
sealed or escrowed**: every per-note secret rederives on the FMan from
its wallet root and the public terms, so the FMan signature over the
response is the sole authenticity mechanism. The quote's identity is
the SHA-256 of the signed response payload bytes — the exact bytes the
FMan signed and the FI verified travel back inside `CreateSeat`, so
both sides hash the same bytes and no canonicalization scheme is
needed; tampering with any byte severs the quote from its identity and
its signature at once. Stateless derivation needs no escrow token, and hashing
the signed bytes needs no canonical form. **The mint generation is never
negotiated**: the payment
federation's own modules decide it. A payment federation is accepted only when
its consensus-signed client config contains exactly one supported Bitcoin mint
generation: one mintv1 module, or one Bitcoin-denominated mintv2 module. A
federation carrying both generations, repeated supported mint modules, no
supported mint, or a non-Bitcoin mintv2 is rejected before its client is first
persisted (and revalidated when an existing client is opened). Both parties
therefore derive the same unambiguous choice from the same config; the FI reads
the FMan's choice off the payment-terms variant. A consumer wallet
states the generation it settled under (`PreparedSeatPayment::settled_under`),
and `fi-client` compares it against the quote's terms before presenting
anything: a wallet that dispatched on something else has locked funds
under the wrong protocol, and the payer catches that rather than the
FMan refusing the presentation. The terms obey one
**coherence rule** with a single shared implementation on the wire
type (`QuoteTerms::check_coherent`): a quote is paid iff it carries a
price, its payment federation is exactly the requested one, and the
price equals the issuance total. The FMan composes terms through the
rule; the FI re-checks a verified quote against it before paying, and
the FMan re-checks it after verifying the echoed signature at
`CreateSeat` before accepting any payment material —
the FI funds the issuance set, so without the check an FMan could
quote an issuance total above the stated price and be overpaid. An FMan may
currently accept several payment federations. Fedi explicitly chooses a funded
payer before Pay-and-create. `fi-client` sends that exact federation in every
quote request, and each quote binds it; the wallet then proves the complete
verified quote set is payable from that same federation before any output is
authorized. FMan policy can still reject a payer that is no longer admitted.
The quote binds the requesting `fi_id`; a free-plan quote carries no payment
terms.

Paid-seat pricing is governed by
owner-directed net-revenue contract is pending implementation: current
operator settings and signed quotes still treat `price` as a configured
gross amount.

Quote-note keys derive statelessly from the wallet root and per-quote
randomness (mintv1: the quote's 32-byte nonce; mintv2: each note's
tweak), **never from a mint client's sequential index
tree** (indices would make quotes stateful, and spam-driven gaps past
the mintv1 recovery gap limit would lose wallet recovery). Issuance-set
uniqueness rests on that randomness — the same assumption as
every generated key — and the quote identity (the hash of the signed
quote bytes) covers the nonce with every other term, so one payment's
evidence can never satisfy two quotes. mintv2 note secrets
follow fedimint's own standard root+tweak derivation
(`StandardDoubleDerive` module root; the tweak is the only per-note
input, so v2 issuance-set uniqueness rests on tweak randomness).
Tweaks are random, carried openly in the quote terms (every v2 output
exposes its tweak on-chain anyway), and deliberately **not ground**
against the mint's private scan filter, so stock scan recovery is
structurally blind to escrow-phase notes. Standard derivation remains a ruling, not a
convenience: **every note locked to FMan keys is the FMan's money, in
escrow phases included**, and every such note stays reachable from the
mnemonic — background settlement moves everything the FMan learns of
into the wallet's own scan-visible notes within seconds, escrowed
value self-heals through material the FI already holds (see *Refusal
and refund*), and in extremis a filterless exhaustive scan derives
any FMan-keyed output from the root and its public on-chain tweak.
Recovery consequences are under *Claiming*, labels under
[ARCH-fleet-manager-identity](./ARCH-fleet-manager-identity.md).

The signed offer epoch identifies the exact `QuoteSettings` snapshot used to
price a quote. A changed quote setting replaces it durably with fresh 32-byte
randomness; an idempotent write does not. Old quotes remain valid indefinitely
while those settings do not change and become permanently stale when they do.

## Payment

The FI pays with one fedimint transaction: inputs its own notes,
outputs the quoted blinded nonces exactly. It waits for consensus,
collects the aggregated blinded signatures, and presents them with the
quote at `CreateSeat`. The presentation carries signatures and nothing
else: which mint protocol was priced is already in the quote's signed
payment terms, so a presented protocol could only agree with it or be
refused, and there is nothing left to disagree. The FMan never learns or
needs the paying transaction's identity: verification and claim work
entirely from the signatures. The denominations fix the amount:
underpayment and overpayment are unrepresentable. Collection needs only public data
(the outputs' denominations and blinded messages), so the FI can
collect signatures for notes it can never spend; for mintv2 the forked
client's `await_output_signatures` does exactly this.

## Acceptance

`CreateSeat` verification is offline and constant-cost: re-derive the
quote's note secrets from the wallet root and the quoted public
randomness (the derivation reproduces the quoted blinded nonces only
for terms this FMan issued), unblind the presented signatures, verify
each note against the payment federation's aggregate mint keys,
require the nonce set to equal the quote's, and reject duplicate
finalized note nonces so every quoted denomination contributes one
independently spendable note to the gross amount. Fleet Manager then prepares
the signed acceptance before admission so signing failure cannot strand a row.
SQLite atomically returns an accepted replay, refuses a stale offer epoch, or
checks capacity and creates the seat facts plus the verified payment's typed
mint-generation evidence in a foreign-keyed claim row. A free seat has no claim
row. The signed acceptance is not stored; an accepted replay freshly signs the
same semantic payload. Persisting the evidence makes hand-off
restart-safe without the FI ever re-presenting. Acceptance never contacts the
payment federation synchronously.

`CreateSeat` is idempotent on the quote: a replay returns the same acceptance
under a fresh signature, or the same refusal and refund material.

## Refusal and refund

A refusal of a **paid** presentation returns a signed refund transaction in the
same response. Its inputs are the verified paid notes and its outputs are the
refund issuance requests bound into the signed quote request. The FI submits it; the FMan background
loop never submits refunds. The FMan verifies the exact mint fee balance before
admission.

No refusal is persisted. An `OfferChanged` refusal compares the quote's signed
offer epoch for equality with a durable epoch replaced atomically by the
acceptance that consumes the last slot or a real `QuoteSettings` change. Each
replacement is fresh 32-byte randomness, so a refused epoch cannot recur except
with negligible 256-bit collision probability. It commits before publication,
so a refusal cannot later reverse. Replays recompute the decision and sign the
quote-determined refund.

## Claiming

The claim row is a durable work item: typed mint-v1 or mint-v2 evidence plus
the payment federation's public invite code and an optional terminal outcome.
The invite makes the evidence self-contained: after mnemonic restore, claiming
can rejoin the payment federation even when it has since left the currently
admitted setup-payment set. `fman-fedimint` reconstructs and verifies the notes,
derives their deterministic receive operation id, checks the Fedimint operation
log first, and only then starts or resumes that operation. Mintv2 resumes only
when the existing operation is a receive whose stored encoded ecash exactly
matches; another operation kind, malformed metadata, or non-identical ecash is
an error. Mint-v1 likewise derives the client's tagged hash before handoff and
checks that the client returns it.

The accepted notes are already FMan-owned at the durable acceptance commit:
the FMan wallet secret is their only secret spend authority, combined with the
public quoted derivation inputs, and the payer holds no spend secret. The durable
seat keeps only lifecycle facts. Its claim row retains the generation-specific
public evidence needed to reconstruct the notes. Immediate reminting is therefore a wallet-integration
choice, not an ownership transfer or a protocol requirement. The current
implementation remints because the current Fedimint client does not provide a
suitable way for its ordinary balance and sweep path to handle retained external
notes directly. A future implementation could instead add such client support or
use a durable FMan payout path that spends the retained external notes as inputs;
this specification requires neither an upstream change nor immediate reminting.

The implementation-owned worker scans nonterminal rows at startup and
periodically, and an acceptance or exact replay wakes it early. Attempts are
bounded in time and concurrency; recoverable failures back off and retry without
requiring an FI request or daemon restart. Fedimint's operation log is the
in-progress journal, so no operation id is duplicated in core. Terminal success
or already-spent inputs and its timestamp remain visible to the operator. Evidence remains
after success as recovery material if the Fedimint client database is lost.

## Wallet requirements

Quoting for and verifying against a payment federation requires exactly
that it is joined (its client config supplies the aggregate mint keys);
the FMan holds one wallet per accepted payment federation, all derived
from the same wallet root.
The FI side needs client support for paying to externally supplied
blinded nonces and finalizing issuance from relayed blinded signatures —
client-side additions only. For mintv2 the fleet consumes fedimint from
the `fedibtc/fedimint` Fedi release line (tag `v0.11.1-fedi16`: upstream
v0.11.1 plus the Fedi MintV2 additions, fee-quote/spendable-amount APIs for
revenue sweeps, and tagged server diagnostics) whose two added
`MintClientModule` methods
carry the locked-payment additions:
`await_output_signatures` collects a transaction's mint-output
signatures from public (denomination, blinded message) pairs — payment
collection and refund finalization, both FI-side — and
`finalize_external_issuance` unblinds relayed signatures and verifies
the notes against the mint's aggregate keys offline (FMan acceptance
and claim). The locked-payment addition itself requires no fedimintd protocol
change; everything else uses the public v0.11.1 API. The design uses real module APIs rather than raw endpoint workarounds and structured methods rather than visibility changes.

## Rejected alternatives

These alternatives remain relevant to the current design tradeoffs.

- **Bearer tokens**: the payer retains spend
  keys, forcing foreground reissue racing the payer's double-spend,
  cross-seat token-hash arbitration, and ambiguous-failure crash
  recovery. Locking notes to the FMan dissolves all three.




- **Key-return refunds** (hand the FI the spend keys): fee-free but
  unadjudicable — once both parties know the keys, theft reports are
  unfalsifiable in both directions; the signed refund transaction is
  attributable.
- **FI-side blinding/unblinding**: whoever blinds owns recovery, so
  this breaks v2 mnemonic scan recovery, leaks note nonces to the payer
  (making the FMan's later spends trackable), and needs more FI crypto
  code — to save one microsecond-scale unblind.
- **Encrypted federation backup for v1 recovery** (stash token secrets
  in the v1 client backup's metadata): workable, deferred — background
  claim plus the database already cover v1.
- **A private quote root for v2 escrow invisibility** (derive v2 note
  secrets from a domain-separated child of the module root so scan
  recovery structurally never imports quote notes): rejected because every
  FMan-keyed note is the FMan's money, and mnemonic-only recovery must
  reach all of it; a private derivation domain would leave paid-but-
  never-presented value invisible to every standard recovery path.
  Un-ground standard tweaks later delivered the same escrow
  invisibility without a second derivation domain: the secret stays
  root plus a public on-chain tweak, so nothing is ever outside the
  mnemonic's reach.
- **Ground tweaks (scan-visible escrow)**: grind each quote tweak (~2^16 hashes per note at
  `GetQuote`, on a blocking thread) so mnemonic scan recovery
  auto-imports escrow-phase notes. Rejected because the auto-import was exactly what created the post-recovery
  double-spend caveat and its rejected mitigation menu, and it bought
  scan visibility only for a sliver that background settlement already
  keeps to seconds — while the FI's own material heals every
  non-orphan case without it. Grinding also coupled `GetQuote` to the
  private scan filter, which must never be shared.
- **A quarantined second v2 wallet for escrowed notes**: v0.11.1 has
  no public API for importing a foreign finalized note into a wallet
  (balance entry is only the fee-bearing receive reissue), so
  quarantine needs that API twice plus its own recovery story.
- **Online acceptance (FI presents only the txid, FMan fetches the
  signatures itself)**: puts the payment federation's availability
  back into the `CreateSeat` path; presenting the aggregated
  signatures keeps acceptance offline and constant-cost, and the FI
  already holds them from its own consensus wait.

## Common setup-payment federation set

Fedi maintains one common set of Fedimint federations for FI-to-FMan setup
payments. Every FI and FMan uses that set. Fedi may update it when a federation
malfunctions or business needs change, but such updates should be infrequent;
the set is expected to remain relatively static during normal operation.

An FMan does not curate its own accepted subset and does not publish a list of
accepted setup-payment federations. The common set already establishes which
federations an FI and every FMan use.

This decision implements the payment-rail requirement in
[REQ-fedimint-setup-payments](../../../specs/REQ-fedimint-setup-payments.md).
The exact mechanism by which Fedi publishes the set is outside this decision.

## Rationale and tradeoffs

Fedimint ecash is federation-specific, so payer and payee must use the same
federation. A relatively static common set makes that overlap predictable for
all FIs and FMan operators while retaining Fedi's ability to respond to a
malfunction or business change.

Having each FMan publish a separate accepted list would require the FI to fetch
and account for those lists across all selected FMans. That adds logic and
failure modes without useful flexibility when all participants are expected to
use the common set.

The common set makes Fedi a central policy authority.
