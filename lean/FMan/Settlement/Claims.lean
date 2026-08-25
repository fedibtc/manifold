import FMan.Settlement.Invariants

/-!
# The quote-settlement conclusions

These theorems state the older refund-ledger model properties summarized in
`crates/fman/specs/CLAIM-fleet-manager-quote-settlement-exclusive/proof.md`, stated over the model in
`FMan.Settlement.Model`. What these theorems do *not* establish is that the model
is a faithful abstraction of the Rust; that is the separate obligation of
the stale correspondence section of that proof.
-/

namespace FMan.Settlement

variable {st : State}

/-! ## Outcome uniqueness -/

/-- No quote ever has both an accepted-seat row and a refund-ledger row. -/
theorem outcome_exclusive (h : Reachable st) (q : QuoteId) :
    ¬((st.durable.seatFor q).isSome ∧ (st.durable.refundFor q).isSome) := by
  rintro ⟨hs, hr⟩
  rcases (inv_reachable h).disjoint q with hn | hn
  · rw [hn] at hs; exact Bool.noConfusion hs
  · rw [hn] at hr; exact Bool.noConfusion hr

/-- At most one seat row per quote. -/
theorem seat_row_unique (h : Reachable st) :
    ∀ x ∈ st.durable.seats, ∀ y ∈ st.durable.seats, x.quote = y.quote → x = y :=
  (inv_reachable h).seatUnique

/-- At most one refund-ledger row per quote. -/
theorem refund_row_unique (h : Reachable st) :
    ∀ x ∈ st.durable.refunds, ∀ y ∈ st.durable.refunds, x.quote = y.quote → x = y :=
  (inv_reachable h).refundUnique

/-- The startup both-outcomes check would detect overlap rather than prevent it,
but no reachable model state trips it. -/
theorem startup_overlap_check_never_fires (h : Reachable st) :
    ∀ q, st.durable.seatFor q = none ∨ st.durable.refundFor q = none :=
  (inv_reachable h).disjoint

/-! ## Refund canonicality -/

/-- The exit channels for refund bytes: the mint submission and the signed
`CreateSeatResponse` commitment. -/
def RefundLeft (st : State) (q : QuoteId) (t : Txn) : Prop :=
  Event.submitRefund q t ∈ st.log ∨ Event.refusalResp q (some t) ∈ st.log

/-- Every refund transaction that left the process for `q` is the committed
ledger row. -/
theorem refund_matches_ledger (h : Reachable st) {q : QuoteId} {t : Txn}
    (hleft : RefundLeft st q t) : ∃ r, st.durable.refundFor q = some r ∧ r.txn = t :=
  (inv_reachable h).refundSound q t hleft

/-- All copies that leave are byte-identical. -/
theorem refund_canonical (h : Reachable st) {q : QuoteId} {t₁ t₂ : Txn}
    (h₁ : RefundLeft st q t₁) (h₂ : RefundLeft st q t₂) : t₁ = t₂ := by
  obtain ⟨r₁, hr₁, ht₁⟩ := refund_matches_ledger h h₁
  obtain ⟨r₂, hr₂, ht₂⟩ := refund_matches_ledger h h₂
  rw [hr₁] at hr₂
  cases hr₂
  rw [← ht₁, ← ht₂]

/-- The refund ordering property. A guard is evaluated against the state in
which its effect runs, so recovering `EffectOk` at a position inside the hold
shows the ledger row was committed strictly before the exit occurred — rather than
merely coexisting with it in a later state.

`Effect.emit` models an *external exit*. The retired correspondence placed that
boundary at the former wallet-interface invocation and signed-response hand-back,
not at network submission or transaction construction. -/
theorem effectOk_at {st : State} {pre : List Effect} {e : Effect} {post : List Effect}
    (h : EffectsOk st (pre ++ e :: post)) : EffectOk (runEffects st pre) e := by
  induction pre generalizing st with
  | nil => cases h with | cons hone _ => exact hone
  | cons a as ih => cases h with | cons _ hrest => exact ih hrest

/-- **Every guarded exit, not just the refund one.** At the point any external exit
runs inside a hold, its guard already holds of the state it runs in. This is the
whole of the old correspondence's exit-ordering condition, stated once; the corollaries
below are the same theorem with the guard unfolded per exit kind, and exist so
that the enumeration of exits is visible rather than left to the reader. -/
theorem exit_backed_at {st : State} (hi : Inv st) (inp : CreateInput) (adm : Admission)
    {pre post : List Effect} {e : Event}
    (hsplit : plan st inp adm = pre ++ .emit e :: post) :
    EffectOk (runEffects st pre) (.emit e) :=
  effectOk_at (st := st) (pre := pre) (e := .emit e) (post := post)
    (hsplit ▸ plan_ok hi inp adm)

/-- Refund submission, fresh or replay: at the moment the transaction leaves for
the mint, its ledger row is already committed. -/
theorem refund_committed_before_emission {st : State} (hi : Inv st)
    (inp : CreateInput) (adm : Admission) {pre post : List Effect} {q : QuoteId} {t : Txn}
    (hsplit : plan st inp adm = pre ++ .emit (.submitRefund q t) :: post) :
    ∃ r, (runEffects st pre).durable.refundFor q = some r ∧ r.txn = t :=
  exit_backed_at hi inp adm hsplit

/-- Refusal response carrying a transaction, fresh or replay: same row, already
committed when the response is handed back. -/
theorem refusal_response_backed_at {st : State} (hi : Inv st)
    (inp : CreateInput) (adm : Admission) {pre post : List Effect} {q : QuoteId} {t : Txn}
    (hsplit : plan st inp adm = pre ++ .emit (.refusalResp q (some t)) :: post) :
    ∃ r, (runEffects st pre).durable.refundFor q = some r ∧ r.txn = t :=
  exit_backed_at hi inp adm hsplit

/-- Acceptance response, fresh or replay: the seat row backing the returned
commitment is already committed when it is returned. -/
theorem accept_response_backed_at {st : State} (hi : Inv st)
    (inp : CreateInput) (adm : Admission) {pre post : List Effect} {q : QuoteId} {s : SeatId}
    (hsplit : plan st inp adm = pre ++ .emit (.acceptResp q s) :: post) :
    ∃ r, (runEffects st pre).durable.seatFor q = some r ∧ r.seat = s :=
  exit_backed_at hi inp adm hsplit

/-- Modeled claim, fresh or replay: the seat row is committed first. -/
theorem claim_backed_at {st : State} (hi : Inv st)
    (inp : CreateInput) (adm : Admission) {pre post : List Effect} {q : QuoteId}
    (hsplit : plan st inp adm = pre ++ .emit (.claim q) :: post) :
    ((runEffects st pre).durable.seatFor q).isSome :=
  exit_backed_at hi inp adm hsplit

/-- The one exit condition 4 is vacuous for: a free refusal returns no transaction
and requires no row. Stated so that "every guarded exit" is exhaustive by
inspection of `EffectOk`, with this the only unguarded case. -/
theorem free_refusal_unguarded (st : State) (q : QuoteId) :
    EffectOk st (.emit (.refusalResp q none)) :=
  trivial

/-! ## Claim only after durable acceptance -/

/-- A claim event for `q` occurs only when its accepted-seat row is durable. -/
theorem claim_after_durable_acceptance (h : Reachable st) (q : QuoteId)
    (hlog : Event.claim q ∈ st.log) : (st.durable.seatFor q).isSome :=
  (inv_reachable h).claimSound q hlog

/-- The returned acceptance commitment is the one bound to the durable row, so a
replay cannot answer with a different seat's commitment. -/
theorem accept_response_matches_row (h : Reachable st) (q : QuoteId) (s : SeatId)
    (hlog : Event.acceptResp q s ∈ st.log) :
    ∃ r, st.durable.seatFor q = some r ∧ r.seat = s :=
  (inv_reachable h).acceptSound q s hlog

/-! ## Claim and refund exclusion -/

/-- The shape of the feared bad thing: one quote whose notes are both claimed
into the FMan wallet and refunded to the FI. -/
theorem no_claim_and_refund (h : Reachable st) (q : QuoteId) (t : Txn)
    (hclaim : Event.claim q ∈ st.log) (hrefund : RefundLeft st q t) : False := by
  have hs := claim_after_durable_acceptance h q hclaim
  obtain ⟨_, hr, _⟩ := refund_matches_ledger h hrefund
  exact outcome_exclusive h q ⟨hs, by rw [hr]; rfl⟩

end FMan.Settlement
