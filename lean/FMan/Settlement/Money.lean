import FMan.Settlement.Claims

/-!
# Conditional mint-settlement results

These theorems derive "Q's locked notes settle value at most once" from the
refund-ledger model plus explicit mint hypotheses. The current Linked Specs
settlement claim deliberately excludes this mint-level conclusion. The theorems
remain useful conditional evidence, but no current model-to-Rust correspondence
discharges their hypotheses.

The derivation has two paths:

* `money_one_settlement` uses the modeled settlement conclusions via
  `daemon_submits_one_transaction` plus spend authority.
* `money_backstop` uses the mint's `singleSpend` premise alone to bound two
  conflicting spends, with no appeal to model reachability.

## What mechanising this turned up

`MintTx` is a *modelled* submission, not a concrete mint transaction, and the two
are not in bijection. The retired implementation could re-invoke payment claiming
on an accepted replay; its own logic did not make the resulting concrete
transactions identical. The mint is therefore modelled by a relation `Submits`,
not a function, and `reissueCanonical` explicitly supplies the missing step from
one modeled submission to one concrete transaction.

That hypothesis is not discharged here, and it corrects the reading this file
carried before: the main path does *not* stand free of a mint guarantee in the
sense that matters. It omits `singleSpend` structurally —
`money_one_settlement` takes `MintBase`,
which has no `singleSpend` field, so it can be instantiated for a mint that
double-spends freely — but only by paying `reissueCanonical` instead. Either the
client's reissue is canonical, or the claim side falls back on `money_backstop`,
which is `singleSpend`. The refund side needs neither, because `MintTx.refund` carries the
transaction and `refund_canonical` is a theorem.

The split into `MintBase` and `MintModel` exists so that this is visible in the
theorems' types rather than only in their proof terms: an audit that reads
`#print axioms` alone would not have caught a `singleSpend` premise that the proof
never projects.
-/

namespace FMan.Settlement

/-- A modeled submission for a quote. This is the daemon-side act, not the
concrete mint transaction it produces; refund submission carries transaction
bytes while claim submission leaves construction to the client. -/
inductive MintTx where
  | claim (q : QuoteId)
  | refund (q : QuoteId) (t : Txn)
  deriving DecidableEq, Repr

def MintTx.quote : MintTx → QuoteId
  | .claim q => q
  | .refund q _ => q

/-- Every modelled daemon submission for `q`. Not a set of mint transactions:
`MintTx.claim q` is the modeled claim act, and `MintBase.Submits` relates it to
the concrete transactions it may produce. -/
def submittedFor (st : State) (q : QuoteId) : List MintTx :=
  st.log.filterMap fun
    | .claim q' => if q' = q then some (.claim q') else none
    | .submitRefund q' t => if q' = q then some (.refund q' t) else none
    | _ => none

theorem mem_submittedFor {st : State} {q : QuoteId} {m : MintTx}
    (h : m ∈ submittedFor st q) :
    (m = .claim q ∧ Event.claim q ∈ st.log) ∨
      (∃ t, m = .refund q t ∧ Event.submitRefund q t ∈ st.log) := by
  obtain ⟨ev, hev, hmap⟩ := List.mem_filterMap.mp h
  cases ev with
  | claim q' =>
    by_cases hq : q' = q
    · subst hq; simp at hmap; exact Or.inl ⟨hmap.symm, hev⟩
    · simp [hq] at hmap
  | submitRefund q' t =>
    by_cases hq : q' = q
    · subst hq; simp at hmap; exact Or.inr ⟨t, hmap.symm, hev⟩
    · simp [hq] at hmap
  | acceptResp => simp at hmap
  | refusalResp => simp at hmap

/-- `no_claim_and_refund` and `refund_canonical` imply at most one distinct
modeled submission per quote. This says nothing about how many concrete mint
transactions a claim submission produces; `reissueCanonical` supplies that
separate premise. -/
theorem daemon_submits_one_transaction {st : State} (h : Reachable st) (q : QuoteId) :
    ∀ x ∈ submittedFor st q, ∀ y ∈ submittedFor st q, x = y := by
  intro x hx y hy
  rcases mem_submittedFor hx with ⟨rfl, hxlog⟩ | ⟨tx, rfl, hxlog⟩ <;>
    rcases mem_submittedFor hy with ⟨rfl, hylog⟩ | ⟨ty, rfl, hylog⟩
  · rfl
  · exact absurd (no_claim_and_refund h q ty hxlog (Or.inl hylog)) not_false
  · exact absurd (no_claim_and_refund h q tx hylog (Or.inl hxlog)) not_false
  · have := refund_canonical h (Or.inl hxlog) (Or.inl hylog)
    subst this; rfl

/-- The mint as the *main path* needs it, with no single-spend assumption. Kept
separate from `MintModel` so that the type of `money_one_settlement` witnesses
that `singleSpend` is not merely unused in the proof term but absent from its premises: a
reader can instantiate `MintBase` for a mint that double-spends freely. -/
structure MintBase where
  Tx : Type
  /-- The set of notes a transaction spends, identified by the quote whose
  issuance produced them. -/
  notes : Tx → QuoteId
  /-- `Submits m x`: `x` is a concrete mint transaction the daemon's submission
  `m` can produce. A relation permits a modeled replay to build a different
  transaction. -/
  Submits : MintTx → Tx → Prop
  /-- Whatever a submission for `q` produces spends `q`'s notes. -/
  submits_notes : ∀ m x, Submits m x → notes x = m.quote
  /-- Whether a transaction settles. Idempotent resubmission is *represented* by
  this being a predicate on the transaction rather than on an
  act of submitting, so two submissions of one `Tx` cannot settle twice. It is not
  thereby established: that the model cannot express two applications of the same
  transaction is a modelling choice, and reading it as "the mint is idempotent"
  remains a trusted semantic premise. -/
  settles : Tx → Prop

/-- `MintBase` plus a single-spend premise. -/
structure MintModel extends MintBase where
  /-- Of two transactions spending a common note set, at most one settles. -/
  singleSpend : ∀ x y, toMintBase.notes x = toMintBase.notes y →
    toMintBase.settles x → toMintBase.settles y → x = y

/-- **Authority-and-canonicality path.** Two hypotheses beyond reachability:

* `authority` — every settling transaction over `q`'s notes is one the daemon
  submitted, which is where spend authority enters;
* `reissueCanonical` — one daemon submission yields at most one concrete
  transaction, which is *not* a consequence of the daemon's logic and is the
  hypothesis a model-to-client correspondence must discharge.

Given both, `daemon_submits_one_transaction` collapses the submissions and
`reissueCanonical` collapses the transactions. `singleSpend` is not used; that is
the whole point of the split, and `money_backstop` is what covers the case where
`reissueCanonical` fails. -/
theorem money_one_settlement (M : MintBase) {st : State} (h : Reachable st) (q : QuoteId)
    (authority : ∀ x : M.Tx, M.notes x = q → M.settles x →
      ∃ m ∈ submittedFor st q, M.Submits m x)
    (reissueCanonical : ∀ (m : MintTx) (x y : M.Tx), M.Submits m x → M.Submits m y → x = y)
    (x y : M.Tx) (hx : M.notes x = q) (hy : M.notes y = q)
    (hsx : M.settles x) (hsy : M.settles y) : x = y := by
  obtain ⟨mx, hmx, hrx⟩ := authority x hx hsx
  obtain ⟨my, hmy, hry⟩ := authority y hy hsy
  refine reissueCanonical mx x y hrx ?_
  rwa [daemon_submits_one_transaction h q mx hmx my hmy]

/-- **Single-spend fallback.** No reachability, `authority`, or
`reissueCanonical` premise is needed. `singleSpend` alone proves the result, so a
consumer that cannot establish canonical reissue loses independence from the
mint. -/
theorem money_one_settlement_of_singleSpend (M : MintModel) (q : QuoteId)
    (x y : M.Tx) (hx : M.notes x = q) (hy : M.notes y = q)
    (hsx : M.settles x) (hsy : M.settles y) : x = y :=
  M.singleSpend x y (hx.trans hy.symm) hsx hsy

/-- **Single-spend backstop.** `singleSpend` bounds a conflicting pair without
the daemon model or `reissueCanonical`. Two concrete claim transactions built by
two replays over `q`'s notes still settle at most once. -/
theorem money_backstop (M : MintModel) (q : QuoteId) (mx my : MintTx) (x y : M.Tx)
    (hsub : M.Submits mx x) (hsub' : M.Submits my y)
    (hx : mx.quote = q) (hy : my.quote = q)
    (hsx : M.settles x) (hsy : M.settles y) : x = y :=
  M.singleSpend _ _ (by rw [M.submits_notes _ _ hsub, M.submits_notes _ _ hsub', hx, hy])
    hsx hsy

end FMan.Settlement
