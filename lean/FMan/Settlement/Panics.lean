import FMan.Settlement.Invariants

/-!
# Where a failure inside the lock hold leaves the state

The retired settlement path had fallible points that the model's `Step` did not
label. Its correspondence had to say what model state each one reached without
assuming whether a panic unwound only the handler task or aborted the process.
The current claim proof marks that correspondence retired and stale.

These lemmas settle that. For a failure at a point where the mirror is already in
step with the seat table, the two panic semantics give the *same* model state,
because the restart rebuild is the identity there. That is the case at every
before-any-write failure and at the registry-insert `.expect`. It is *not* the
case between the seat commit and the mirror insertion, and
`mirror_lags_at_prefix_one` exhibits that gap rather than hiding it.
-/

namespace FMan.Settlement

variable {st : State}

/-- Rebuilding the modeled acceptance mirror changes nothing when it is complete. -/
theorem reboot_noop_of_mirror (h : st.vol.accepted = st.durable.seats) : st.reboot = st := by
  simp [State.reboot, rebuild, ← h]

/-- **Modeled stutter points.** A failure before any effect of the retired hold
ran left the state untouched. If the process died there, restart reached the same
state, so the old correspondence could take no model step. -/
theorem reboot_noop (hi : Inv st) : st.reboot = st :=
  reboot_noop_of_mirror hi.mirror

/-- **Failure after mirror insertion.** Once the modeled seat and acceptance
mirror exist, the state is not a stutter but is restart-stable. The two-effect
prefix is mirror-consistent under either task or process abort. -/
theorem panic_after_mirror_insert (hi : Inv st) (inp : CreateInput)
    (hacc : st.vol.acceptedFor inp.quote = none)
    (href : st.durable.refundFor inp.quote = none) :
    (runEffects st ((plan st inp .admit).take 2)).reboot
      = runEffects st ((plan st inp .admit).take 2) := by
  apply reboot_noop_of_mirror
  simp [plan, hacc, href, runEffects, Effect.apply, hi.mirror]

/-- **The gap this does not cover.** One effect in, the seat row is durable and the
mirror is not yet updated, and there the rebuild is *not* the identity. A failure
between modeled seat insertion and mirror insertion that unwound only the task
would leave a state satisfying no `Inv`; process abort is `Step.createCrash 1`.
Any renewed correspondence must account for the analogous current window. -/
theorem mirror_lags_at_prefix_one (hi : Inv st) (inp : CreateInput)
    (hacc : st.vol.acceptedFor inp.quote = none)
    (href : st.durable.refundFor inp.quote = none) :
    (runEffects st ((plan st inp .admit).take 1)).vol.accepted
      ≠ (runEffects st ((plan st inp .admit).take 1)).durable.seats := by
  simp [plan, hacc, href, runEffects, Effect.apply, hi.mirror]

end FMan.Settlement
