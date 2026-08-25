import FMan.GuardianDataLoss.Invariants

/-!
# The published-guardian-data model conclusion

The theorem is conditional on the explicit fedimintd/lifecycle contract A2.
It establishes the per-seat event-ordering property of the model, not the Rust
enumerations, filesystem path isolation, or correspondence between model steps
and all implementation executions.
-/

namespace FMan.GuardianDataLoss

/-- On every A2-admissible reachable trace, no data-directory wipe occurs
chronologically after an invite has been emitted to the FI. Since logs are
newest first, `pre` is exactly the portion occurring after that invite. -/
theorem no_wipe_after_invite
    (allowed : State → Probe → Prop)
    (a2 : ∀ st p, allowed st p → FedimintdA2 st p) {st : State}
    (reachable : Reachable allowed st) (pre post : List Event)
    (split : st.log = pre ++ Event.invite :: post) : Event.wipe ∉ pre :=
  (inv_reachable a2 reachable).logSafe pre post split

/-- Equivalent bad-trace shape, convenient for correspondence audits. -/
theorem no_invite_then_wipe
    (allowed : State → Probe → Prop)
    (a2 : ∀ st p, allowed st p → FedimintdA2 st p) {st : State}
    (reachable : Reachable allowed st) :
    ¬ ∃ pre middle post,
      st.log = pre ++ Event.wipe :: middle ++ Event.invite :: post := by
  rintro ⟨pre, middle, post, split⟩
  apply no_wipe_after_invite allowed a2 reachable (pre ++ Event.wipe :: middle) post
  · simpa [List.append_assoc] using split
  · simp

end FMan.GuardianDataLoss
