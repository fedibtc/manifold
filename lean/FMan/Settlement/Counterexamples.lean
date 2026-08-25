import FMan.Settlement.Claims

/-!
# The model can fail

A model property must be able to fail. These are executable witnesses that the
settlement theorems are not vacuous: each section mutates one mechanism the
retired refund-ledger correspondence treated as load-bearing, and exhibits a
concrete reachable trace in the mutated model with two durable outcomes for one
quote.

If a future change to `Model.lean` made the real theorems provable for the wrong
reason, these witnesses would stop being counterexamples and this file would stop
compiling.
-/

namespace FMan.Settlement.Counterexamples

open FMan.Settlement

/-- A paid quote: price is irrelevant here, only that a refusal has refund
material to commit. -/
def inp : CreateInput := { quote := 7, fi := 1, seat := 3, payment := some (5, 9) }

/-! ## Mutation 1 — the startup rebuild is dropped

Drop the modeled acceptance-mirror rebuild from durable seats. The replay branch
then stops recognising a quote after restart, allowing a refund on top of an
existing seat row. -/

namespace NoRebuild

def reboot (st : State) : State := { st with vol := { accepted := [] } }

inductive Step : State → State → Prop where
  | create (inp : CreateInput) (adm : Admission) : Step st (runEffects st (plan st inp adm))
  | crash : Step st (reboot st)

inductive Reachable : State → Prop where
  | init : Reachable Initial
  | step {s s' : State} : Reachable s → Step s s' → Reachable s'

/-- Accept, crash, then replay the same quote and refuse it. -/
def accepted : State := runEffects Initial (plan Initial inp .admit)
def restarted : State := reboot accepted
def refused : State := runEffects restarted (plan restarted inp .refuse)

theorem refused_reachable : Reachable refused :=
  .step (.step (.step .init (.create inp .admit)) .crash) (.create inp .refuse)

/-- Both outcomes exist for quote 7: the FI holds a usable seat *and* a settled
refund. This violates `outcome_exclusive`. -/
theorem double_outcome :
    (refused.durable.seatFor 7).isSome = true ∧ (refused.durable.refundFor 7).isSome = true := by
  constructor <;> rfl

/-- And the refund bytes left the process. -/
theorem refund_left : Event.submitRefund 7 9 ∈ refused.log := by decide

end NoRebuild

/-! ## Mutation 2 — decommission releases the quote

Make decommission remove the modeled acceptance entry. The same double
outcome appears without any crash at all. -/

namespace DecommissionReleases

def decommission (st : State) (s : SeatId) : State :=
  { st with vol := { accepted := st.vol.accepted.filter (fun r => r.seat != s) } }

inductive Step : State → State → Prop where
  | create (inp : CreateInput) (adm : Admission) : Step st (runEffects st (plan st inp adm))
  | decommission (s : SeatId) : Step st (decommission st s)

inductive Reachable : State → Prop where
  | init : Reachable Initial
  | step {s s' : State} : Reachable s → Step s s' → Reachable s'

def accepted : State := runEffects Initial (plan Initial inp .admit)
def retired : State := decommission accepted 3
def refused : State := runEffects retired (plan retired inp .refuse)

theorem refused_reachable : Reachable refused :=
  .step (.step (.step .init (.create inp .admit)) (.decommission 3)) (.create inp .refuse)

theorem double_outcome :
    (refused.durable.seatFor 7).isSome = true ∧ (refused.durable.refundFor 7).isSome = true := by
  constructor <;> rfl

end DecommissionReleases

/-! ## The unmutated model rejects both traces

Neither state above is reachable in `FMan.Settlement.Step`, because
`outcome_exclusive` rules out *any* reachable state with two outcomes. -/

theorem no_double_outcome_in_real_model {st : State} (h : Reachable st) (q : QuoteId)
    (hseat : (st.durable.seatFor q).isSome) (hrefund : (st.durable.refundFor q).isSome) :
    False :=
  outcome_exclusive h q ⟨hseat, hrefund⟩

end FMan.Settlement.Counterexamples
