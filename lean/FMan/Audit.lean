import Lean
import FMan.Settlement.Claims
import FMan.Settlement.Panics
import FMan.Settlement.Money
import FMan.GuardianDataLoss.Claims

/-!
# Axiom audit

This file regenerates, at build time, the exact trusted base of every headline
theorem and fails the build if any depends on `sorryAx`.

The expected output is Lean's own three foundational axioms — `propext`,
`Classical.choice`, `Quot.sound` — and nothing else. Every FMan-specific axiom is
a hypothesis or a structure field rather than an `axiom` command, so it shows up
in the theorem statement rather than here. The mint model exposes
`MintModel.singleSpend`, represents resubmission through `MintBase.settles`, and
uses `MintBase.submits_notes`; `money_one_settlement` additionally takes
`authority` and `reissueCanonical`.

**This audit is therefore not sufficient on its own.** A premise the proof term
never projects is invisible here while still being required to instantiate the
theorem. Splitting `MintBase` from `MintModel` prevents `singleSpend` from hiding
that way in `money_one_settlement`. Read the statements, not only this output.
-/

open Lean

namespace FMan.Audit

/-- Settlement and conditional money theorems whose axiom dependencies are audited. -/
def headline : List Name :=
  [``FMan.Settlement.outcome_exclusive,
   ``FMan.Settlement.seat_row_unique,
   ``FMan.Settlement.refund_row_unique,
   ``FMan.Settlement.startup_overlap_check_never_fires,
   ``FMan.Settlement.refund_matches_ledger,
   ``FMan.Settlement.refund_canonical,
   ``FMan.Settlement.exit_backed_at,
   ``FMan.Settlement.refund_committed_before_emission,
   ``FMan.Settlement.refusal_response_backed_at,
   ``FMan.Settlement.accept_response_backed_at,
   ``FMan.Settlement.claim_backed_at,
   ``FMan.Settlement.free_refusal_unguarded,
   ``FMan.Settlement.claim_after_durable_acceptance,
   ``FMan.Settlement.accept_response_matches_row,
   ``FMan.Settlement.no_claim_and_refund,
   ``FMan.Settlement.daemon_submits_one_transaction,
   ``FMan.Settlement.money_one_settlement,
   ``FMan.Settlement.money_backstop,
   ``FMan.Settlement.money_one_settlement_of_singleSpend,
   ``FMan.Settlement.reboot_noop,
   ``FMan.Settlement.panic_after_mirror_insert,
   ``FMan.Settlement.mirror_lags_at_prefix_one,
   ``FMan.GuardianDataLoss.no_wipe_after_invite,
   ``FMan.GuardianDataLoss.no_invite_then_wipe]

end FMan.Audit

run_cmd do
  let mut bad := #[]
  for n in FMan.Audit.headline do
    let axs ← Elab.Command.liftCoreM <| collectAxioms n
    logInfo m!"{n} depends on axioms: {axs.toList}"
    if axs.contains ``sorryAx then
      bad := bad.push n
  unless bad.isEmpty do
    throwError "these theorems depend on sorryAx: {bad.toList}"
