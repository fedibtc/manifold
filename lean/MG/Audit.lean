import Lean
import MG.Settlement.Claims
import MG.Settlement.Panics
import MG.Settlement.Money
import MG.GuardianDataLoss.Claims

/-!
# Axiom audit

This file regenerates, at build time, the exact trusted base of every headline
theorem and fails the build if any depends on `sorryAx`.

The expected output is Lean's own three foundational axioms — `propext`,
`Classical.choice`, `Quot.sound` — and nothing else. Every MG-specific axiom is
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

namespace MG.Audit

/-- Settlement and conditional money theorems whose axiom dependencies are audited. -/
def headline : List Name :=
  [``MG.Settlement.outcome_exclusive,
   ``MG.Settlement.seat_row_unique,
   ``MG.Settlement.refund_row_unique,
   ``MG.Settlement.startup_overlap_check_never_fires,
   ``MG.Settlement.refund_matches_ledger,
   ``MG.Settlement.refund_canonical,
   ``MG.Settlement.exit_backed_at,
   ``MG.Settlement.refund_committed_before_emission,
   ``MG.Settlement.refusal_response_backed_at,
   ``MG.Settlement.accept_response_backed_at,
   ``MG.Settlement.claim_backed_at,
   ``MG.Settlement.free_refusal_unguarded,
   ``MG.Settlement.claim_after_durable_acceptance,
   ``MG.Settlement.accept_response_matches_row,
   ``MG.Settlement.no_claim_and_refund,
   ``MG.Settlement.daemon_submits_one_transaction,
   ``MG.Settlement.money_one_settlement,
   ``MG.Settlement.money_backstop,
   ``MG.Settlement.money_one_settlement_of_singleSpend,
   ``MG.Settlement.reboot_noop,
   ``MG.Settlement.panic_after_mirror_insert,
   ``MG.Settlement.mirror_lags_at_prefix_one,
   ``MG.GuardianDataLoss.no_wipe_after_invite,
   ``MG.GuardianDataLoss.no_invite_then_wipe]

end MG.Audit

run_cmd do
  let mut bad := #[]
  for n in MG.Audit.headline do
    let axs ← Elab.Command.liftCoreM <| collectAxioms n
    logInfo m!"{n} depends on axioms: {axs.toList}"
    if axs.contains ``sorryAx then
      bad := bad.push n
  unless bad.isEmpty do
    throwError "these theorems depend on sorryAx: {bad.toList}"
