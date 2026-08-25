import FMan.Settlement.Model

/-!
# Settlement invariants

The inductive invariant behind three headline model conclusions:

* `outcome_exclusive`,
* `refund_canonical`,
* `claim_after_durable_acceptance`.

The proof uses a per-effect guard (`EffectOk`), a discharge obligation for one
lock hold (`plan_ok`), and preservation under crash-truncation plus the startup
rebuild.
-/

namespace FMan.Settlement

/-! ## Keyed lookup -/

section Keyed

variable {α : Type} (key : α → QuoteId)

theorem key_of_find {l : List α} {q : QuoteId} {a : α}
    (h : l.find? (fun x => key x == q) = some a) : key a = q := by
  simpa using List.find?_some h

theorem find_none {l : List α} {q : QuoteId}
    (h : l.find? (fun x => key x == q) = none) : ∀ a ∈ l, key a ≠ q := by
  intro a ha
  simpa using (List.find?_eq_none.mp h) a ha

theorem find_eq_some_of_unique {l : List α} {q : QuoteId} {a : α}
    (ha : a ∈ l) (hq : key a = q)
    (huniq : ∀ x ∈ l, ∀ y ∈ l, key x = key y → x = y) :
    l.find? (fun x => key x == q) = some a := by
  cases hfind : l.find? (fun x => key x == q) with
  | none => exact absurd hq (find_none key hfind a ha)
  | some b =>
    have hb := List.mem_of_find?_eq_some hfind
    have hbq := key_of_find key hfind
    rw [huniq b hb a ha (by rw [hbq, hq])]

end Keyed

/-! ## Lookup lemmas over the durable tables -/

theorem seatFor_cons_self (d : Durable) (q : QuoteId) (s : SeatId) (fi : FiId) :
    Durable.seatFor { d with seats := ⟨q, s, fi⟩ :: d.seats } q = some ⟨q, s, fi⟩ := by
  simp [Durable.seatFor]

theorem seatFor_cons_ne {d : Durable} {q0 q : QuoteId} {s : SeatId} {fi : FiId} (h : q0 ≠ q) :
    Durable.seatFor { d with seats := ⟨q0, s, fi⟩ :: d.seats } q = d.seatFor q := by
  simp [Durable.seatFor, h]

theorem seatFor_cons_isSome {d : Durable} {q0 q : QuoteId} {s : SeatId} {fi : FiId}
    (h : (d.seatFor q).isSome) :
    (Durable.seatFor { d with seats := ⟨q0, s, fi⟩ :: d.seats } q).isSome := by
  by_cases hq : q0 = q
  · simp [Durable.seatFor, hq]
  · simpa [seatFor_cons_ne hq] using h

theorem refundFor_cons_self (d : Durable) (q : QuoteId) (fed : FedId) (t : Txn) :
    Durable.refundFor { d with refunds := ⟨q, fed, t⟩ :: d.refunds } q = some ⟨q, fed, t⟩ := by
  simp [Durable.refundFor]

theorem refundFor_cons_ne {d : Durable} {q0 q : QuoteId} {fed : FedId} {t : Txn} (h : q0 ≠ q) :
    Durable.refundFor { d with refunds := ⟨q0, fed, t⟩ :: d.refunds } q = d.refundFor q := by
  simp [Durable.refundFor, h]

theorem seatFor_mem {d : Durable} {q : QuoteId} {r : SeatRow} (h : d.seatFor q = some r) :
    r ∈ d.seats ∧ r.quote = q :=
  ⟨List.mem_of_find?_eq_some h, key_of_find _ h⟩

theorem refundFor_mem {d : Durable} {q : QuoteId} {r : RefundRow} (h : d.refundFor q = some r) :
    r ∈ d.refunds ∧ r.quote = q :=
  ⟨List.mem_of_find?_eq_some h, key_of_find _ h⟩

/-! ## The invariant -/

/-- The durable-and-log half of the invariant. It survives a mid-hold crash,
which is why it is separated from `mirror`. -/
structure Base (st : State) : Prop where
  /-- `seats.quote_id` is `UNIQUE`; at most one seat row per quote. -/
  seatUnique : ∀ x ∈ st.durable.seats, ∀ y ∈ st.durable.seats, x.quote = y.quote → x = y
  /-- `refund_ledger.quote_id` is the primary key. -/
  refundUnique : ∀ x ∈ st.durable.refunds, ∀ y ∈ st.durable.refunds, x.quote = y.quote → x = y
  /-- No quote has both outcomes. -/
  disjoint : ∀ q, st.durable.seatFor q = none ∨ st.durable.refundFor q = none
  /-- Every claim submitted for a quote was preceded by that quote's durable
  seat row (`claim_after_durable_acceptance`). -/
  claimSound : ∀ q, Event.claim q ∈ st.log → (st.durable.seatFor q).isSome
  /-- Every returned acceptance commitment is the one bound to the durable row. -/
  acceptSound : ∀ q s, Event.acceptResp q s ∈ st.log →
      ∃ r, st.durable.seatFor q = some r ∧ r.seat = s
  /-- Every refund transaction that left the process — in the signed response or
  as a mint submission — equals the committed ledger row
  (`refund_canonical`). -/
  refundSound : ∀ q t,
      (Event.submitRefund q t ∈ st.log ∨ Event.refusalResp q (some t) ∈ st.log) →
      ∃ r, st.durable.refundFor q = some r ∧ r.txn = t

/-- The full invariant. `mirror` is the retired acceptance mirror's completeness
with respect to durable seats. It can lapse inside a modeled hold and is restored
by modeled restart. -/
structure Inv (st : State) : Prop extends Base st where
  mirror : st.vol.accepted = st.durable.seats

/-! ## Per-effect guards -/

/-- The precondition each effect needs to preserve `Base`; `plan` discharges each
one through modeled replay lookups and serialized allocation. -/
def EffectOk (st : State) : Effect → Prop
  | .dbCreateSeat q _ _ => st.durable.seatFor q = none ∧ st.durable.refundFor q = none
  | .dbRecordRefund q _ _ => st.durable.seatFor q = none ∧ st.durable.refundFor q = none
  | .volInsertAccepted _ _ _ => True
  | .emit (.claim q) => (st.durable.seatFor q).isSome
  | .emit (.acceptResp q s) => ∃ r, st.durable.seatFor q = some r ∧ r.seat = s
  | .emit (.submitRefund q t) => ∃ r, st.durable.refundFor q = some r ∧ r.txn = t
  | .emit (.refusalResp q (some t)) => ∃ r, st.durable.refundFor q = some r ∧ r.txn = t
  | .emit (.refusalResp _ none) => True

inductive EffectsOk : State → List Effect → Prop where
  | nil {st} : EffectsOk st []
  | cons {st e es} : EffectOk st e → EffectsOk (e.apply st) es → EffectsOk st (e :: es)

theorem base_apply {st : State} {e : Effect} (hb : Base st) (hok : EffectOk st e) :
    Base (e.apply st) := by
  cases e with
  | dbCreateSeat q s fi =>
    obtain ⟨hseat, hrefund⟩ := hok
    have hfresh : ∀ x ∈ st.durable.seats, x.quote ≠ q := find_none _ hseat
    refine ⟨?_, hb.refundUnique, ?_, ?_, ?_, hb.refundSound⟩
    · intro x hx y hy hxy
      simp only [Effect.apply, List.mem_cons] at hx hy
      rcases hx with rfl | hx <;> rcases hy with rfl | hy
      · rfl
      · exact absurd hxy.symm (hfresh y hy)
      · exact absurd hxy (hfresh x hx)
      · exact hb.seatUnique x hx y hy hxy
    · intro q'
      by_cases hq : q' = q
      · subst hq; exact Or.inr hrefund
      · rcases hb.disjoint q' with h | h
        · exact Or.inl (by simpa [Effect.apply, seatFor_cons_ne (Ne.symm hq)] using h)
        · exact Or.inr h
    · intro q' hq'
      exact seatFor_cons_isSome (hb.claimSound q' hq')
    · intro q' s' hq'
      obtain ⟨r, hr, hrs⟩ := hb.acceptSound q' s' hq'
      have : q ≠ q' := fun h => by rw [h, hr] at hseat; exact Option.noConfusion hseat
      exact ⟨r, by simpa [Effect.apply, seatFor_cons_ne this] using hr, hrs⟩
  | dbRecordRefund q fed t =>
    obtain ⟨hseat, hrefund⟩ := hok
    have hfresh : ∀ x ∈ st.durable.refunds, x.quote ≠ q := find_none _ hrefund
    refine ⟨hb.seatUnique, ?_, ?_, hb.claimSound, hb.acceptSound, ?_⟩
    · intro x hx y hy hxy
      simp only [Effect.apply, List.mem_cons] at hx hy
      rcases hx with rfl | hx <;> rcases hy with rfl | hy
      · rfl
      · exact absurd hxy.symm (hfresh y hy)
      · exact absurd hxy (hfresh x hx)
      · exact hb.refundUnique x hx y hy hxy
    · intro q'
      by_cases hq : q' = q
      · subst hq; exact Or.inl hseat
      · rcases hb.disjoint q' with h | h
        · exact Or.inl h
        · exact Or.inr (by simpa [Effect.apply, refundFor_cons_ne (Ne.symm hq)] using h)
    · intro q' t' hq'
      obtain ⟨r, hr, hrt⟩ := hb.refundSound q' t' hq'
      have : q ≠ q' := fun h => by rw [h, hr] at hrefund; exact Option.noConfusion hrefund
      exact ⟨r, by simpa [Effect.apply, refundFor_cons_ne this] using hr, hrt⟩
  | volInsertAccepted q s fi =>
    exact ⟨hb.seatUnique, hb.refundUnique, hb.disjoint, hb.claimSound, hb.acceptSound,
      hb.refundSound⟩
  | emit ev =>
    refine ⟨hb.seatUnique, hb.refundUnique, hb.disjoint, ?_, ?_, ?_⟩
    · intro q hq
      rcases List.mem_cons.mp hq with h | h
      · subst h; simpa [Effect.apply] using hok
      · exact hb.claimSound q h
    · intro q s hq
      rcases List.mem_cons.mp hq with h | h
      · subst h; simpa [Effect.apply] using hok
      · exact hb.acceptSound q s h
    · intro q t hq
      rcases hq with h | h <;> rcases List.mem_cons.mp h with h' | h'
      · subst h'; simpa [Effect.apply] using hok
      · exact hb.refundSound q t (Or.inl h')
      · subst h'; simpa [Effect.apply] using hok
      · exact hb.refundSound q t (Or.inr h')

theorem runEffects_cons (st : State) (e : Effect) (es : List Effect) :
    runEffects st (e :: es) = runEffects (e.apply st) es := rfl

theorem base_runEffects {st : State} {es : List Effect} (hok : EffectsOk st es) :
    Base st → Base (runEffects st es) := by
  induction hok with
  | nil => exact fun hb => hb
  | cons hone _ ih => exact fun hb => by rw [runEffects_cons]; exact ih (base_apply hb hone)


theorem effectsOk_take {st : State} {es : List Effect} (h : EffectsOk st es) :
    ∀ n, EffectsOk st (es.take n) := by
  induction h with
  | nil => intro n; simpa using EffectsOk.nil
  | cons hone _ ih =>
    intro n
    cases n with
    | zero => simpa using EffectsOk.nil
    | succ m => simpa using EffectsOk.cons hone (ih m)

/-! ## One lock hold discharges every guard -/

theorem acceptedFor_eq_seatFor {st : State} (h : st.vol.accepted = st.durable.seats)
    (q : QuoteId) : st.vol.acceptedFor q = st.durable.seatFor q := by
  simp [Volatile.acceptedFor, Durable.seatFor, h]

theorem plan_ok {st : State} (hi : Inv st) (inp : CreateInput) (adm : Admission) :
    EffectsOk st (plan st inp adm) := by
  have hmir := acceptedFor_eq_seatFor hi.mirror inp.quote
  unfold plan
  cases hacc : st.vol.acceptedFor inp.quote with
  | some row =>
    -- Accepted replay: the mirror entry witnesses the durable row.
    have hseat : st.durable.seatFor inp.quote = some row := by rw [← hmir, hacc]
    have hrow : row.seat = row.seat := rfl
    refine .cons ?_ (.cons ?_ .nil)
    · simp [EffectOk, hseat]
    · exact ⟨row, by simpa [Effect.apply] using hseat, hrow⟩
  | none =>
    have hseatNone : st.durable.seatFor inp.quote = none := by rw [← hmir, hacc]
    simp only []
    cases href : st.durable.refundFor inp.quote with
    | some r =>
      -- Refund replay: every copy is the stored ledger row.
      refine .cons ?_ (.cons ?_ .nil)
      · exact ⟨r, href, rfl⟩
      · exact ⟨r, by simpa [Effect.apply] using href, rfl⟩
    | none =>
      cases adm with
      | refuse =>
        cases hpay : inp.payment with
        | none => exact .cons trivial .nil
        | some fedTxn =>
          obtain ⟨fed, t⟩ := fedTxn
          refine .cons ⟨hseatNone, href⟩ (.cons ?_ (.cons ?_ .nil))
          · exact ⟨⟨inp.quote, fed, t⟩, refundFor_cons_self _ _ _ _, rfl⟩
          · exact ⟨⟨inp.quote, fed, t⟩, by
              simpa [Effect.apply] using refundFor_cons_self st.durable inp.quote fed t, rfl⟩
      | admit =>
        refine .cons ⟨hseatNone, href⟩ (.cons trivial (.cons ?_ (.cons ?_ .nil)))
        · simpa [EffectOk, Effect.apply] using
            congrArg Option.isSome (seatFor_cons_self st.durable inp.quote inp.seat inp.fi)
        · exact ⟨⟨inp.quote, inp.seat, inp.fi⟩, by
            simpa [Effect.apply] using
              seatFor_cons_self st.durable inp.quote inp.seat inp.fi, rfl⟩

/-! ## Preservation -/

theorem inv_reboot {st : State} (hb : Base st) : Inv st.reboot :=
  { seatUnique := hb.seatUnique, refundUnique := hb.refundUnique, disjoint := hb.disjoint,
    claimSound := hb.claimSound, acceptSound := hb.acceptSound, refundSound := hb.refundSound,
    mirror := rfl }

theorem inv_create {st : State} (hi : Inv st) (inp : CreateInput) (adm : Admission) :
    Inv (runEffects st (plan st inp adm)) := by
  refine { toBase := base_runEffects (plan_ok hi inp adm) hi.toBase, mirror := ?_ }
  have hmir := acceptedFor_eq_seatFor hi.mirror inp.quote
  unfold plan
  cases hacc : st.vol.acceptedFor inp.quote with
  | some row => simpa [runEffects, Effect.apply] using hi.mirror
  | none =>
    simp only []
    cases href : st.durable.refundFor inp.quote with
    | some r => simpa [runEffects, Effect.apply] using hi.mirror
    | none =>
      cases adm with
      | refuse =>
        cases hpay : inp.payment with
        | none => simpa [runEffects, Effect.apply] using hi.mirror
        | some fedTxn =>
          obtain ⟨fed, t⟩ := fedTxn
          simpa [runEffects, Effect.apply] using hi.mirror
      | admit => simpa [runEffects, Effect.apply] using hi.mirror

theorem inv_step {s s' : State} (hi : Inv s) (hstep : Step s s') : Inv s' := by
  cases hstep with
  | create inp adm => exact inv_create hi inp adm
  | createCrash inp adm n =>
    exact inv_reboot (base_runEffects (effectsOk_take (plan_ok hi inp adm) n) hi.toBase)
  | crash => exact inv_reboot hi.toBase
  | decommission s =>
    exact { seatUnique := hi.seatUnique, refundUnique := hi.refundUnique,
            disjoint := hi.disjoint, claimSound := hi.claimSound,
            acceptSound := hi.acceptSound, refundSound := hi.refundSound,
            mirror := hi.mirror }

theorem inv_initial : Inv Initial := by
  refine { seatUnique := by simp [Initial], refundUnique := by simp [Initial],
           disjoint := by simp [Initial, Durable.seatFor], claimSound := by simp [Initial],
           acceptSound := by simp [Initial], refundSound := by simp [Initial],
           mirror := rfl }

theorem inv_reachable {st : State} (h : Reachable st) : Inv st := by
  induction h with
  | init => exact inv_initial
  | step _ hstep ih => exact inv_step ih hstep

end FMan.Settlement
